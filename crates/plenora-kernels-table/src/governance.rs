//! Governance: asserzioni, riconciliazione e regole dichiarative.
//!
//! Asserzioni `table.assert_cardinality`, `table.assert_metadata`,
//! `table.assert_foreign_key`; `table.reconcile` fra due tabelle;
//! `table.validate_rules`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use plenora_core::arrow::array::{
    builder::StringBuilder, Array, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array,
    RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::{DataType, Field, Schema};
use serde::Deserialize;

use crate::aggregation::BinaryKeyEncoder;
use crate::hashing::FastHasher;
use crate::interning::KeyInterner;
use crate::Limits;
use crate::{
    column_index, compare_f64, compare_i64, reject_rows, replace_or_append, scalar_as_string,
    scalar_compare, NumericBound, RowRejection,
};
use plenora_core::{PlenoraError, Result};

/// Config di `table.assert_cardinality`.
///
/// L'analisi del contratto pretende almeno un vincolo, rifiuta `exact_rows`
/// insieme a `min_rows`/`max_rows`, `min_rows > max_rows` e i vincoli oltre
/// `max_rows` dei limiti.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertCardinality {
    /// Numero esatto di righe; esclude gli altri due.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub exact_rows: Option<usize>,
    /// Minimo di righe, incluso; almeno 1 (0 non vincola nulla).
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub min_rows: Option<usize>,
    /// Massimo di righe, incluso.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub max_rows: Option<usize>,
}

impl AssertCardinality {
    /// Regole sulla sola config, condivise da kernel e analisi dei
    /// contratti: almeno un vincolo; `exact_rows` da solo; `min_rows` non
    /// oltre `max_rows`; `min_rows = 0` vale per ogni tabella, quindi non
    /// avrebbe effetto e si rifiuta. I limiti dei vincoli contro `max_rows`
    /// dei limiti li controlla l'analisi.
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        let piano = |messaggio: &str| Err(PlenoraError::InvalidPlan(messaggio.to_owned()));
        if self.exact_rows.is_none() && self.min_rows.is_none() && self.max_rows.is_none() {
            return piano("assert_cardinality richiede exact_rows, min_rows o max_rows");
        }
        if self.exact_rows.is_some() && (self.min_rows.is_some() || self.max_rows.is_some()) {
            return piano("exact_rows non si combina con min_rows o max_rows");
        }
        if self.min_rows == Some(0) {
            return piano("min_rows 0 non ha effetto: ogni tabella ha almeno 0 righe");
        }
        if self
            .min_rows
            .zip(self.max_rows)
            .is_some_and(|(min, max)| min > max)
        {
            return piano("limiti di assert_cardinality non validi");
        }
        Ok(())
    }
}

/// Batch invariato se il numero di righe rispetta il contratto dichiarato.
///
/// Nessuna diagnostica per riga: il difetto e' della tabella, non di una
/// riga.
///
/// # Errors
///
/// - `InvalidPlan`: le regole di [`AssertCardinality::verifica_parametri`];
///   righe diverse da `exact_rows`, oppure fuori dall'intervallo
///   `min_rows`/`max_rows`.
pub fn assert_cardinality(batch: &RecordBatch, config: &AssertCardinality) -> Result<RecordBatch> {
    config.verifica_parametri()?;
    let valid = config.exact_rows.map_or_else(
        || {
            config.min_rows.is_none_or(|min| batch.num_rows() >= min)
                && config.max_rows.is_none_or(|max| batch.num_rows() <= max)
        },
        |exact| batch.num_rows() == exact,
    );
    if valid {
        Ok(batch.clone())
    } else {
        Err(PlenoraError::InvalidPlan(format!(
            "assert_cardinality: {} righe fuori contratto",
            batch.num_rows()
        )))
    }
}

/// Config di `table.assert_metadata`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertMetadata {
    /// Coppie chiave-valore che i metadati di schema devono contenere, con
    /// lo stesso valore (obbligatorio). L'analisi del contratto rifiuta la
    /// mappa vuota, oltre `max_columns` coppie, chiavi vuote e testi oltre
    /// `max_string_bytes`.
    pub expected: BTreeMap<String, String>,
    /// Con `true` (default) i metadati possono avere altre chiavi; con
    /// `false` hanno esattamente quelle di `expected`.
    #[serde(default = "default_true")]
    pub allow_extra: bool,
}

const fn default_true() -> bool {
    true
}

/// Batch invariato se i metadati dello schema contengono tutte le coppie
/// `expected` (con gli stessi valori).
///
/// Guarda solo lo schema: nel runner l'analisi del contratto rifiuta gli
/// stessi casi in validazione, con `InvalidPlan`.
///
/// # Errors
///
/// - `Schema`: chiave attesa assente o con valore diverso; con
///   `allow_extra = false`, anche numero di metadati diverso da `expected`.
pub fn assert_metadata(batch: &RecordBatch, config: &AssertMetadata) -> Result<RecordBatch> {
    let schema = batch.schema();
    let metadata = schema.metadata();
    if (!config.allow_extra && metadata.len() != config.expected.len())
        || config
            .expected
            .iter()
            .any(|(key, value)| metadata.get(key) != Some(value))
    {
        return Err(PlenoraError::Schema(
            "assert_metadata: metadata non conforme".into(),
        ));
    }
    Ok(batch.clone())
}

/// Config di `table.assert_foreign_key`.
///
/// Le chiavi si abbinano per posizione; l'analisi del contratto pretende
/// liste non vuote, della stessa lunghezza, senza ripetizioni, con colonne
/// leggibili come testo scalare e tipi identici per coppia.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignKey {
    /// Colonne della chiave esterna nella tabella sinistra (obbligatorio).
    pub left_keys: Vec<String>,
    /// Colonne della chiave riferita nella tabella destra (obbligatorio).
    pub right_keys: Vec<String>,
    /// Con `true` una riga sinistra con un null nella chiave passa senza
    /// controllo; con `false` (default) e' rifiutata.
    #[serde(default)]
    pub allow_null: bool,
}

fn key_indices(batch: &RecordBatch, names: &[String]) -> Result<Vec<usize>> {
    names.iter().map(|name| column_index(batch, name)).collect()
}

fn validate_key_types(
    left: &RecordBatch,
    right: &RecordBatch,
    left_indices: &[usize],
    right_indices: &[usize],
) -> Result<()> {
    if left_indices
        .iter()
        .zip(right_indices)
        .any(|(left_index, right_index)| {
            left.column(*left_index).data_type() != right.column(*right_index).data_type()
        })
    {
        return Err(PlenoraError::Schema(
            "foreign key richiede tipi Arrow identici".into(),
        ));
    }
    Ok(())
}

/// `true` se una delle colonne indicate e' nulla nella riga.
///
/// Null LOGICO: `reconcile` e `assert_foreign_key` decidono da qui se una
/// riga partecipa al confronto, e una dictionary con chiave valida verso
/// una entry nulla parteciperebbe come se avesse un valore.
fn has_null(batch: &RecordBatch, indices: &[usize], row: usize) -> bool {
    indices
        .iter()
        .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
}

// ---------------------------------------------------------------------------
// Chiavi testuali di riga: l'oracolo dei test di `reconcile` e
// `assert_foreign_key`.
//
// `RowKeyEncoder` scrive in un buffer riusato gli stessi byte di
// `quality::key_for_row`. Era il percorso di `reconcile` e
// `assert_foreign_key`; ora serve ai test, come copia del percorso
// precedente e come misura dei byte che `LatoChiavi::lunghezza_testuale`
// deve rendere.
// ---------------------------------------------------------------------------

/// Colonna di chiave tipizzata per `RowKeyEncoder`.
#[cfg(test)]
enum KeyValueColumn<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    UInt64(&'a UInt64Array),
    Generic(&'a ArrayRef),
}

#[cfg(test)]
impl<'a> KeyValueColumn<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<BooleanArray>() {
            return Self::Boolean(values);
        }
        if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        Self::Generic(array)
    }

    /// Scrive in `text` gli stessi byte di `scalar_as_string` per `row`;
    /// restituisce `false` (senza scrivere) se il valore e' null.
    fn write_value(&self, row: usize, text: &mut String) -> Result<bool> {
        match self {
            Self::Utf8(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                text.push_str(values.value(row));
            }
            Self::Int64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato.
                write!(text, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato.
                write!(text, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::Boolean(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato.
                write!(text, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::UInt64(values) => {
                if values.is_null(row) {
                    return Ok(false);
                }
                // `fmt::Write` su `String` e' infallibile; l'errore e'
                // comunque propagato come Internal, mai ignorato.
                write!(text, "{}", values.value(row))
                    .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
            }
            Self::Generic(array) => {
                let Some(value) = scalar_as_string(array.as_ref(), row)? else {
                    return Ok(false);
                };
                text.push_str(&value);
            }
        }
        Ok(true)
    }
}

/// Encoder zero-copy di chiavi di riga.
///
/// Stessi byte di `quality::key_for_row` senza allocare una String per
/// colonna per riga (`reconcile`, `assert_foreign_key`).
#[cfg(test)]
pub(crate) struct RowKeyEncoder<'a> {
    columns: Vec<(Vec<u8>, KeyValueColumn<'a>)>,
    text: String,
}

#[cfg(test)]
impl<'a> RowKeyEncoder<'a> {
    pub(crate) fn new(batch: &'a RecordBatch, indices: &[usize]) -> Self {
        let columns = indices
            .iter()
            .map(|index| {
                let column = batch.column(*index);
                let type_name = column.data_type().to_string();
                let mut prefix = Vec::with_capacity(8 + type_name.len());
                prefix.extend_from_slice(&(type_name.len() as u64).to_be_bytes());
                prefix.extend_from_slice(type_name.as_bytes());
                (prefix, KeyValueColumn::new(column))
            })
            .collect();
        Self {
            columns,
            text: String::new(),
        }
    }

    /// Scrive in `output` (riusato tra le righe) gli stessi byte di
    /// `quality::key_for_row` per `row`.
    pub(crate) fn encode_into(&mut self, row: usize, output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        for (prefix, column) in &self.columns {
            output.extend_from_slice(prefix);
            self.text.clear();
            if column.write_value(row, &mut self.text)? {
                output.push(1);
                output.extend_from_slice(&(self.text.len() as u64).to_be_bytes());
                output.extend_from_slice(self.text.as_bytes());
            } else {
                output.push(0);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Chiavi di riga di `reconcile` e `assert_foreign_key`.
//
// Identita' binaria di `BinaryKeyEncoder` in un `KeyInterner`: due righe
// hanno la stessa chiave se e solo se hanno gli stessi byte di
// `quality::key_for_row`, perche' `validate_key_types` pretende tipi
// identici fra i due lati e la codifica binaria coincide con quella
// testuale a tipo fissato. Su una sola colonna Int64 la chiave e' il valore
// nativo, con il null come chiave a parte.
//
// La contabilita' della memoria resta quella delle chiavi testuali:
// `LatoChiavi::lunghezza_testuale` rende la lunghezza dei byte di
// `key_for_row` senza costruirli, e la si addebita (piu' 64 byte) alla prima
// comparsa della chiave in un lato, come quando la mappa conteneva quei
// byte. Le soglie di `max_governed_memory_bytes` e `max_rows` scattano alla
// stessa riga.
// ---------------------------------------------------------------------------

/// Lunghezza del testo di `scalar_as_string` di una colonna di chiave, per
/// tipo; `None` sul null.
enum LunghezzaValore<'a> {
    Utf8(&'a StringArray),
    Int64(&'a Int64Array),
    UInt64(&'a UInt64Array),
    Float64(&'a Float64Array),
    Boolean(&'a BooleanArray),
    /// Byte grezzi: per un binario UTF-8 valido la lunghezza del testo, per
    /// uno non valido quella dei byte che `BinaryKeyEncoder` codifica.
    Binary(&'a BinaryArray),
    Generica(&'a ArrayRef),
}

/// Cifre decimali di `valore`.
const fn cifre_decimali(mut valore: u64) -> usize {
    let mut cifre = 1;
    while valore >= 10 {
        valore /= 10;
        cifre += 1;
    }
    cifre
}

impl<'a> LunghezzaValore<'a> {
    fn new(array: &'a ArrayRef) -> Self {
        let any = array.as_any();
        if let Some(values) = any.downcast_ref::<StringArray>() {
            return Self::Utf8(values);
        }
        if let Some(values) = any.downcast_ref::<Int64Array>() {
            return Self::Int64(values);
        }
        if let Some(values) = any.downcast_ref::<UInt64Array>() {
            return Self::UInt64(values);
        }
        if let Some(values) = any.downcast_ref::<Float64Array>() {
            return Self::Float64(values);
        }
        if let Some(values) = any.downcast_ref::<BooleanArray>() {
            return Self::Boolean(values);
        }
        if let Some(values) = any.downcast_ref::<BinaryArray>() {
            return Self::Binary(values);
        }
        Self::Generica(array)
    }

    /// Byte del testo della cella (`Display` per i tipi nativi, come
    /// `scalar_as_string`), `None` se nulla. `testo` e' un buffer riusato.
    fn lunghezza(&self, row: usize, testo: &mut String) -> Result<Option<usize>> {
        Ok(match self {
            Self::Utf8(values) => (!values.is_null(row)).then(|| values.value(row).len()),
            Self::Int64(values) => (!values.is_null(row)).then(|| {
                let valore = values.value(row);
                usize::from(valore < 0) + cifre_decimali(valore.unsigned_abs())
            }),
            Self::UInt64(values) => {
                (!values.is_null(row)).then(|| cifre_decimali(values.value(row)))
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    None
                } else {
                    testo.clear();
                    // `fmt::Write` su `String` e' infallibile; l'errore e'
                    // comunque propagato come Internal, mai ignorato.
                    write!(testo, "{}", values.value(row))
                        .map_err(|_| PlenoraError::Internal("fmt su String".into()))?;
                    Some(testo.len())
                }
            }
            Self::Boolean(values) => (!values.is_null(row))
                .then(|| if values.value(row) { "true" } else { "false" }.len()),
            Self::Binary(values) => (!values.is_null(row)).then(|| values.value(row).len()),
            Self::Generica(array) => {
                scalar_as_string(array.as_ref(), row)?.map(|testo| testo.len())
            }
        })
    }
}

/// Un lato di `reconcile`/`assert_foreign_key`: encoder binario, colonna
/// Int64 nativa se la chiave e' una sola colonna Int64, lunghezze testuali.
struct LatoChiavi<'a> {
    batch: &'a RecordBatch,
    indices: &'a [usize],
    encoder: BinaryKeyEncoder<'a>,
    int64: Option<&'a Int64Array>,
    /// Per colonna: byte fissi del frammento testuale (lunghezza del nome del
    /// tipo a 8 byte, nome, marcatore di null) e lunghezza del valore.
    colonne: Vec<(usize, LunghezzaValore<'a>)>,
}

impl<'a> LatoChiavi<'a> {
    fn new(batch: &'a RecordBatch, indices: &'a [usize]) -> Self {
        let int64 = match indices {
            [index] => batch.column(*index).as_any().downcast_ref::<Int64Array>(),
            _ => None,
        };
        let colonne = indices
            .iter()
            .map(|index| {
                let column = batch.column(*index);
                let fissi = 8 + column.data_type().to_string().len() + 1;
                (fissi, LunghezzaValore::new(column))
            })
            .collect();
        Self {
            batch,
            indices,
            encoder: BinaryKeyEncoder::new(batch, indices),
            int64,
            colonne,
        }
    }

    /// Lunghezza dei byte di `quality::key_for_row` per `row`: per colonna i
    /// byte fissi, piu' 8 byte di lunghezza e il testo se la cella non e'
    /// nulla.
    fn lunghezza_testuale(&self, row: usize, testo: &mut String) -> Result<usize> {
        let mut totale = 0_usize;
        for (fissi, valore) in &self.colonne {
            let variabili = valore
                .lunghezza(row, testo)?
                .map_or(0, |lunghezza| lunghezza.saturating_add(8));
            totale = totale.saturating_add(fissi.saturating_add(variabili));
        }
        Ok(totale)
    }
}

/// Indici densi delle chiavi di riga, in ordine di prima apparizione.
enum IndiceChiavi {
    /// Una sola colonna Int64 su entrambi i lati: il valore nativo, il null
    /// a parte.
    Int64 {
        valori: HashMap<i64, usize, FastHasher>,
        nullo: Option<usize>,
        prossimo: usize,
    },
    /// Chiave binaria di `BinaryKeyEncoder`; `buffer` e' riusato.
    Binario {
        chiavi: KeyInterner,
        buffer: Vec<u8>,
    },
}

impl IndiceChiavi {
    /// Indice nativo se **entrambi** i lati hanno una sola colonna Int64,
    /// binario altrimenti. `validate_key_types` confronta solo le coppie di
    /// colonne, non il loro numero: `["id"]` contro `["id", "grp"]` e' un
    /// piano valido, e li' le chiavi non coincidono mai (la codifica di una
    /// colonna e' autodelimitata), come nel percorso testuale.
    fn per(sinistra: &LatoChiavi<'_>, destra: &LatoChiavi<'_>, capacita: usize) -> Self {
        if sinistra.int64.is_some() && destra.int64.is_some() {
            Self::Int64 {
                valori: HashMap::with_capacity_and_hasher(capacita, FastHasher::default()),
                nullo: None,
                prossimo: 0,
            }
        } else {
            Self::Binario {
                chiavi: KeyInterner::with_capacity(capacita),
                buffer: Vec::new(),
            }
        }
    }

    /// Numero di chiavi distinte.
    const fn len(&self) -> usize {
        match self {
            Self::Int64 { prossimo, .. } => *prossimo,
            Self::Binario { chiavi, .. } => chiavi.len(),
        }
    }

    /// Indice della chiave di `row` e `true` se e' nuova.
    ///
    /// # Errors
    ///
    /// Gli errori di conversione di `BinaryKeyEncoder`; `Internal` se il lato
    /// non ha la colonna Int64 dell'indice nativo.
    fn inserisci(&mut self, lato: &LatoChiavi<'_>, row: usize) -> Result<(usize, bool)> {
        match self {
            Self::Int64 {
                valori,
                nullo,
                prossimo,
            } => {
                let values = lato.int64.ok_or_else(senza_int64)?;
                let candidato = *prossimo;
                let indice = if values.is_null(row) {
                    *nullo.get_or_insert(candidato)
                } else {
                    *valori.entry(values.value(row)).or_insert(candidato)
                };
                let nuova = indice == candidato;
                if nuova {
                    *prossimo += 1;
                }
                Ok((indice, nuova))
            }
            Self::Binario { chiavi, buffer } => {
                lato.encoder.encode_into(row, buffer)?;
                Ok(chiavi.inserisci(buffer))
            }
        }
    }

    /// Indice della chiave di `row`, se presente.
    ///
    /// # Errors
    ///
    /// Come [`IndiceChiavi::inserisci`].
    fn cerca(&mut self, lato: &LatoChiavi<'_>, row: usize) -> Result<Option<usize>> {
        match self {
            Self::Int64 { valori, nullo, .. } => {
                let values = lato.int64.ok_or_else(senza_int64)?;
                Ok(if values.is_null(row) {
                    *nullo
                } else {
                    valori.get(&values.value(row)).copied()
                })
            }
            Self::Binario { chiavi, buffer } => {
                lato.encoder.encode_into(row, buffer)?;
                Ok(chiavi.cerca(buffer))
            }
        }
    }
}

fn senza_int64() -> PlenoraError {
    PlenoraError::Internal("indice di chiavi Int64 su un lato senza colonna Int64".into())
}

/// Batch sinistro invariato se ogni chiave sinistra e' referenziata nella
/// tabella destra.
///
/// Una chiave con un null in una qualunque colonna e' nulla: a destra non
/// si registra, a sinistra decide `allow_null`. La memoria contata e' quella
/// delle chiavi distinte della destra: lunghezza della forma testuale di
/// `key_for_row` piu' 64 byte per chiave.
///
/// # Errors
///
/// - `Schema`: colonna chiave assente (in `left` o `right`); tipi Arrow
///   delle chiavi non identici fra i due lati; cella di chiave fuori dai
///   tipi nativi che non si converte in testo (come `scalar_as_string`);
/// - `DataMapping` con diagnostica per riga, senza colonna: chiave nulla in
///   `left` con `allow_null = false` (causa `validation.foreign_key_null`) o
///   chiave sinistra assente da `right` (causa
///   `validation.foreign_key_missing`);
/// - `ResourceLimit`: memoria delle chiavi destre oltre
///   `limits.max_governed_memory_bytes`, o traboccamento del suo contatore;
/// - `Internal`: invarianti interne violate (indice nativo Int64 senza la
///   colonna, contatori delle diagnostiche).
pub fn assert_foreign_key(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &ForeignKey,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_indices = key_indices(left, &config.left_keys)?;
    let right_indices = key_indices(right, &config.right_keys)?;
    validate_key_types(left, right, &left_indices, &right_indices)?;
    let destra = LatoChiavi::new(right, &right_indices);
    let sinistra = LatoChiavi::new(left, &left_indices);
    let mut referenced = IndiceChiavi::per(&sinistra, &destra, right.num_rows());
    let mut memory_used = 0_usize;
    let mut testo = String::new();
    for row in 0..right.num_rows() {
        if !has_null(right, &right_indices, row) && referenced.inserisci(&destra, row)?.1 {
            let key_bytes = destra.lunghezza_testuale(row, &mut testo)?;
            memory_used = memory_used
                .checked_add(key_bytes.saturating_add(64))
                .ok_or_else(|| {
                    PlenoraError::ResourceLimit("overflow memoria foreign key".into())
                })?;
            if memory_used > limits.max_governed_memory_bytes {
                return Err(PlenoraError::ResourceLimit(
                    "assert_foreign_key oltre max_governed_memory_bytes".into(),
                ));
            }
        }
    }
    let mut rejections = Vec::new();
    for row in 0..left.num_rows() {
        if has_null(left, &left_indices, row) {
            if config.allow_null {
                continue;
            }
            rejections.push(RowRejection {
                row,
                cause: "validation.foreign_key_null",
                column: None,
            });
            continue;
        }
        if referenced.cerca(&sinistra, row)?.is_none() {
            rejections.push(RowRejection {
                row,
                cause: "validation.foreign_key_missing",
                column: None,
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(left.clone())
}

/// Config di `table.reconcile`.
///
/// Le chiavi si abbinano per posizione; l'analisi del contratto pretende
/// liste non vuote, della stessa lunghezza, senza ripetizioni, con colonne
/// leggibili come testo scalare e tipi identici per coppia.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconcile {
    /// Colonne della chiave nella tabella sinistra (obbligatorio).
    pub left_keys: Vec<String>,
    /// Colonne della chiave nella tabella destra (obbligatorio).
    pub right_keys: Vec<String>,
    /// Con `true` (default) il null e' un valore della chiave e due null si
    /// abbinano; con `false` una riga con un null nella chiave conta fra le
    /// righe solo del suo lato, e non fra i duplicati.
    #[serde(default = "default_true")]
    pub nulls_equal: bool,
}

/// Frequenze delle chiavi di un lato di `reconcile`, per indice di
/// `chiavi` (condiviso fra i due lati: una chiave presente nei due ha lo
/// stesso indice).
///
/// La memoria e il limite di `max_rows` contano le chiavi distinte **del
/// lato**, alla prima comparsa nel lato, come la mappa per lato di prima:
/// una chiave gia' vista nell'altro lato si addebita di nuovo.
fn frequencies(
    lato: &LatoChiavi<'_>,
    chiavi: &mut IndiceChiavi,
    nulls_equal: bool,
    side_nulls: &mut usize,
    memory_used: &mut usize,
    limits: &Limits,
) -> Result<Vec<usize>> {
    let mut conteggi: Vec<usize> = Vec::new();
    let mut distinte = 0_usize;
    let mut testo = String::new();
    for row in 0..lato.batch.num_rows() {
        if !nulls_equal && has_null(lato.batch, lato.indices, row) {
            *side_nulls = side_nulls.checked_add(1).ok_or_else(|| {
                PlenoraError::ResourceLimit("overflow null reconciliation".into())
            })?;
            continue;
        }
        let (indice, _) = chiavi.inserisci(lato, row)?;
        if indice >= conteggi.len() {
            conteggi.resize(indice + 1, 0);
        }
        let conteggio = conteggi
            .get_mut(indice)
            .ok_or_else(|| PlenoraError::Internal("reconcile: conteggio assente".into()))?;
        if *conteggio == 0 {
            *memory_used = memory_used
                .checked_add(lato.lunghezza_testuale(row, &mut testo)?.saturating_add(64))
                .ok_or_else(|| {
                    PlenoraError::ResourceLimit("overflow memoria reconciliation".into())
                })?;
            if *memory_used > limits.max_governed_memory_bytes {
                return Err(PlenoraError::ResourceLimit(
                    "reconcile oltre max_governed_memory_bytes".into(),
                ));
            }
            distinte += 1;
            if distinte > limits.max_rows {
                return Err(PlenoraError::ResourceLimit(
                    "reconcile supera max_rows chiavi distinte".into(),
                ));
            }
        }
        *conteggio = conteggio
            .checked_add(1)
            .ok_or_else(|| PlenoraError::ResourceLimit("overflow reconciliation".into()))?;
    }
    Ok(conteggi)
}

fn as_u64(value: usize) -> Result<u64> {
    u64::try_from(value).map_err(|_| PlenoraError::ResourceLimit("conteggio oltre u64".into()))
}

/// Resoconto di riconciliazione fra due tabelle, in cinque metriche.
///
/// Un batch di cinque righe `metric` (`Utf8`) / `value` (`UInt64`), in
/// quest'ordine: `matched_rows`, `left_only_rows`, `right_only_rows`,
/// `left_duplicate_rows`, `right_duplicate_rows`.
///
/// Per ogni chiave con `L` righe a sinistra e `R` a destra: abbinate
/// `min(L, R)`, solo a sinistra `L - min(L, R)`, solo a destra
/// `R - min(L, R)`, duplicati `L - 1` e `R - 1` sulle chiavi presenti in quel
/// lato. Con `nulls_equal = false` le righe con una chiave nulla si sommano
/// alle righe solo del loro lato.
///
/// # Errors
///
/// - `Schema`: colonna chiave assente (in `left` o `right`); tipi Arrow
///   delle chiavi non identici fra i due lati; cella di chiave fuori dai
///   tipi nativi che non si converte in testo (come `scalar_as_string`);
/// - `ResourceLimit`: memoria delle chiavi oltre
///   `limits.max_governed_memory_bytes` (chiavi distinte contate lato per
///   lato, lunghezza della forma testuale piu' 64 byte); chiavi distinte di
///   un lato oltre `limits.max_rows`; conteggio oltre `u64` o traboccamento
///   dei contatori;
/// - `Internal`: invarianti interne violate;
/// - `DataMapping` (`arrow error`): errore Arrow nella costruzione del batch
///   di output.
pub fn reconcile(
    left: &RecordBatch,
    right: &RecordBatch,
    config: &Reconcile,
    limits: &Limits,
) -> Result<RecordBatch> {
    let left_indices = key_indices(left, &config.left_keys)?;
    let right_indices = key_indices(right, &config.right_keys)?;
    validate_key_types(left, right, &left_indices, &right_indices)?;
    let sinistra = LatoChiavi::new(left, &left_indices);
    let destra = LatoChiavi::new(right, &right_indices);
    let mut chiavi = IndiceChiavi::per(&sinistra, &destra, left.num_rows());
    let mut left_nulls = 0;
    let mut right_nulls = 0;
    let mut memory_used = 0_usize;
    let left_counts = frequencies(
        &sinistra,
        &mut chiavi,
        config.nulls_equal,
        &mut left_nulls,
        &mut memory_used,
        limits,
    )?;
    let right_counts = frequencies(
        &destra,
        &mut chiavi,
        config.nulls_equal,
        &mut right_nulls,
        &mut memory_used,
        limits,
    )?;
    let mut matched = 0_usize;
    let mut left_only = left_nulls;
    let mut right_only = right_nulls;
    let mut left_duplicates = 0_usize;
    let mut right_duplicates = 0_usize;
    // Stesse somme delle due visite delle mappe per lato: un conteggio nullo
    // e' una chiave assente da quel lato.
    for indice in 0..chiavi.len() {
        let left_count = left_counts.get(indice).copied().unwrap_or_default();
        let right_count = right_counts.get(indice).copied().unwrap_or_default();
        if left_count > 0 {
            let common = left_count.min(right_count);
            matched = matched.saturating_add(common);
            left_only = left_only.saturating_add(left_count - common);
            left_duplicates = left_duplicates.saturating_add(left_count.saturating_sub(1));
        }
        if right_count > 0 {
            right_only = right_only.saturating_add(right_count.saturating_sub(left_count));
            right_duplicates = right_duplicates.saturating_add(right_count.saturating_sub(1));
        }
    }
    let metrics = [
        "matched_rows",
        "left_only_rows",
        "right_only_rows",
        "left_duplicate_rows",
        "right_duplicate_rows",
    ];
    let values = [
        matched,
        left_only,
        right_only,
        left_duplicates,
        right_duplicates,
    ]
    .into_iter()
    .map(as_u64)
    .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("metric", DataType::Utf8, false),
            Field::new("value", DataType::UInt64, false),
        ])),
        vec![
            Arc::new(StringArray::from(metrics.to_vec())),
            Arc::new(UInt64Array::from(values)),
        ],
    )?)
}

// ---------------------------------------------------------------------------
// table.validate_rules
// ---------------------------------------------------------------------------

/// Operatore di una regola di validazione: sottoinsieme degli operatori di
/// `filtering` con nomi testuali stabili (in minuscolo nella config).
///
/// "Colonna numerica" qui e' una di [`is_rule_numeric`]: `Utf8` non lo e'.
/// Una cella nulla fa fallire ogni operatore tranne `isnull`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleOperator {
    /// `eq`: uguale a `value`. Esatto nel dominio nativo su una colonna
    /// numerica (`value` numerico; su `Float64` `NaN` e' uguale a `NaN`),
    /// testuale su `Utf8`, `Boolean` e `Binary`.
    Eq,
    /// `ne`: negazione di `eq`; una cella che non si legge fa fallire
    /// entrambi.
    Ne,
    /// `gt`: maggiore di `value`; colonna numerica, `value` numerico.
    Gt,
    /// `ge`: maggiore o uguale; come `gt`.
    Ge,
    /// `lt`: minore; come `gt`.
    Lt,
    /// `le`: minore o uguale; come `gt`.
    Le,
    /// `isnull`: cella nulla (null logico); qualunque colonna, senza
    /// `value`.
    Isnull,
    /// `notnull`: cella non nulla; qualunque colonna, senza `value`.
    Notnull,
    /// `regex`: la regex `value` trova una corrispondenza nel testo
    /// (ricerca, non corrispondenza intera); colonna `Utf8`.
    Regex,
    /// `range`: `value` e' il testo `"min,max"`, estremi inclusi; colonna
    /// numerica.
    Range,
}

/// Gravita' di una regola: decide in quale elenco (o conteggio) finisce la
/// sua violazione.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSeverity {
    /// `error` (default): la violazione rende la riga non valida.
    #[default]
    Error,
    /// `warning`: la violazione si segnala, la riga resta valida.
    Warning,
}

/// Forma dell'uscita di [`validate_rules`].
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidateOutputMode {
    /// Per riga: colonne `_valid`/`_errors`/`_warnings` aggiunte all'input.
    #[default]
    Annotate,
    /// Una riga aggregata per regola: `name`, `errors`, `warnings`.
    Summary,
}

/// Una regola di [`validate_rules`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateRule {
    /// Nome della regola nell'uscita (obbligatorio): non vuoto, unico fra le
    /// regole; l'analisi del contratto lo limita a 1024 byte.
    pub name: String,
    /// Operatore (obbligatorio).
    pub operator: RuleOperator,
    /// Colonna su cui si valuta la regola. Facoltativa per serde, ma una
    /// regola senza `column` si rifiuta con `InvalidPlan`.
    #[serde(default)]
    pub column: Option<String>,
    /// Termine di confronto: obbligatorio per ogni operatore tranne
    /// `isnull`/`notnull`, dove non e' ammesso. Una stringa vale il suo
    /// testo, un altro valore il suo testo JSON; `null` vale assente.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// Gravita' (default `error`).
    #[serde(default)]
    pub severity: RuleSeverity,
}

/// Config di `table.validate_rules`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateRules {
    /// Regole, valutate nell'ordine scritto (obbligatorio, almeno una;
    /// l'analisi del contratto ne ammette al piu' `max_columns`).
    pub rules: Vec<ValidateRule>,
    /// Forma dell'uscita (default `annotate`).
    #[serde(default)]
    pub output_mode: ValidateOutputMode,
}

/// Regola compilata: tutti i controlli statici (colonna, tipo-operatore,
/// valore atteso, regex) sono gia' stati fatti, prima di toccare i dati.
struct CompiledRule {
    name: String,
    operator: RuleOperator,
    column_index: usize,
    numeric_column: bool,
    severity: RuleSeverity,
    expected: String,
    /// Estremi in forma esatta (letterale intero preservato).
    ///
    /// Sono l'UNICA forma degli estremi numerici: una copia in `f64` che li
    /// affiancasse collasserebbe gli interi oltre 2^53.
    expected_bound: Option<NumericBound>,
    expected_high_bound: Option<NumericBound>,
    regex: Option<regex::Regex>,
}

/// Replica `json_text` del kernel filtering.
fn rule_json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Il valore numerico di una regola, letto esatto.
///
/// Vale per `eq`/`ne` su colonna numerica e per `gt`/`ge`/`lt`/`le`, come lo
/// confronta il kernel ([`NumericBound::parse`]); `None` se non e' un numero
/// o se la forma esatta non lo tiene (`1e-128`). Autorita' unica per il kernel e l'analisi
/// dei contratti: un valore accettato dall'una e rifiutato dall'altro
/// sarebbe un errore di config visto solo in esecuzione.
#[must_use]
pub fn valore_regola(testo: &str) -> Option<NumericBound> {
    NumericBound::parse(testo)
}

/// Gli estremi di una regola `range`, letti esatti.
///
/// Il testo e' `"min,max"`, spazi attorno ammessi, e ogni estremo si legge
/// come [`valore_regola`]: `None` senza la virgola,
/// `Some(None)` se un estremo non e' un numero esatto.
#[must_use]
pub fn estremi_regola(testo: &str) -> Option<Option<(NumericBound, NumericBound)>> {
    let (low, high) = testo.split_once(',')?;
    Some(valore_regola(low.trim()).zip(valore_regola(high.trim())))
}

/// Tipi su cui i confronti ordinati/range hanno senso.
///
/// Leggibili da `scalar_as_f64`, escluso `Utf8` (un testo da parsare per
/// riga non e' una colonna numerica: meglio un errore in validazione che un
/// fallimento per riga). Riusata dall'analisi a secco del contratto.
#[must_use]
pub const fn is_rule_numeric(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Float64
            | DataType::Int64
            | DataType::UInt64
            | DataType::Date32
            | DataType::Timestamp(plenora_core::arrow::schema::TimeUnit::Millisecond, _)
            | DataType::Decimal128(_, _)
    )
}

/// Tipi confrontabili per uguaglianza (profilo scalare testuale o numerico).
/// Riusata dall'analisi a secco del contratto.
#[must_use]
pub const fn is_rule_comparable(data_type: &DataType) -> bool {
    is_rule_numeric(data_type)
        || matches!(
            data_type,
            DataType::Utf8 | DataType::Boolean | DataType::Binary
        )
}

/// Compila le regole con tutti i controlli statici.
///
/// Ogni errore di configurazione (colonna mancante, tipo incompatibile con
/// l'operatore, valore atteso non parsabile, regex invalida) esce QUI, prima
/// di leggere una sola riga di dati.
// Dispatcher esaustivo per operatore: la lunghezza e' nei casi, non nella logica.
#[allow(clippy::too_many_lines)]
fn compile_rules(batch: &RecordBatch, config: &ValidateRules) -> Result<Vec<CompiledRule>> {
    if config.rules.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "validate_rules richiede almeno una regola".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut compiled = Vec::with_capacity(config.rules.len());
    for rule in &config.rules {
        if rule.name.trim().is_empty() {
            return Err(PlenoraError::InvalidPlan(
                "validate_rules: nome regola vuoto".into(),
            ));
        }
        if !seen.insert(rule.name.as_str()) {
            return Err(PlenoraError::InvalidPlan(format!(
                "validate_rules: regola ripetuta: {}",
                rule.name
            )));
        }
        let column = rule.column.as_deref().ok_or_else(|| {
            PlenoraError::InvalidPlan(format!("validate_rules: regola {} senza column", rule.name))
        })?;
        let column_index = column_index(batch, column)?;
        let data_type = batch.column(column_index).data_type().clone();
        let needs_value = !matches!(rule.operator, RuleOperator::Isnull | RuleOperator::Notnull);
        if needs_value != rule.value.is_some() {
            return Err(PlenoraError::InvalidPlan(format!(
                "validate_rules: regola {}: value {} per l'operatore {:?}",
                rule.name,
                if needs_value {
                    "obbligatorio"
                } else {
                    "non ammesso"
                },
                rule.operator
            )));
        }
        let expected = rule.value.as_ref().map_or_else(String::new, rule_json_text);
        let mut expected_bound = None;
        let mut expected_high_bound = None;
        let mut regex = None;
        let numeric_column = is_rule_numeric(&data_type);
        match rule.operator {
            RuleOperator::Isnull | RuleOperator::Notnull => {}
            RuleOperator::Eq | RuleOperator::Ne => {
                if !is_rule_comparable(&data_type) {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: tipo {data_type:?} non confrontabile",
                        rule.name
                    )));
                }
                if numeric_column {
                    expected_bound = Some(valore_regola(&expected).ok_or_else(|| {
                        PlenoraError::InvalidPlan(format!(
                            "validate_rules: regola {}: confronto numerico con valore non numerico",
                            rule.name
                        ))
                    })?);
                }
            }
            RuleOperator::Gt | RuleOperator::Ge | RuleOperator::Lt | RuleOperator::Le => {
                if !numeric_column {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: confronto ordinato richiede colonna numerica (tipo {data_type:?})",
                        rule.name
                    )));
                }
                expected_bound = Some(valore_regola(&expected).ok_or_else(|| {
                    PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: confronto ordinato richiede un valore numerico",
                        rule.name
                    ))
                })?);
            }
            RuleOperator::Range => {
                if !numeric_column {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: range richiede colonna numerica (tipo {data_type:?})",
                        rule.name
                    )));
                }
                let Some(estremi) = estremi_regola(&expected) else {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: range richiede min,max",
                        rule.name
                    )));
                };
                let (low, high) = estremi.ok_or_else(|| {
                    PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: estremi range non numerici",
                        rule.name
                    ))
                })?;
                expected_bound = Some(low);
                expected_high_bound = Some(high);
            }
            RuleOperator::Regex => {
                if data_type != DataType::Utf8 {
                    return Err(PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: regex richiede colonna Utf8 (tipo {data_type:?})",
                        rule.name
                    )));
                }
                regex = Some(regex::Regex::new(&expected).map_err(|error| {
                    PlenoraError::InvalidPlan(format!(
                        "validate_rules: regola {}: regex non valida: {error}",
                        rule.name
                    ))
                })?);
            }
        }
        compiled.push(CompiledRule {
            name: rule.name.clone(),
            operator: rule.operator,
            column_index,
            numeric_column,
            severity: rule.severity,
            expected,
            expected_bound,
            expected_high_bound,
            regex,
        });
    }
    Ok(compiled)
}

/// `range`: il valore non e' sotto il minimo ne' sopra il massimo; `None`
/// (estremo NaN) esclude la riga, come i confronti IEEE storici.
const fn within_rule_range(low: Option<Ordering>, high: Option<Ordering>) -> bool {
    !matches!(low, None | Some(Ordering::Less)) && !matches!(high, None | Some(Ordering::Greater))
}

/// Confronto ordinato di regola da un `Ordering` tipizzato.
///
/// `None` (NaN) rende falso ogni confronto, come IEEE. Restituisce `None` se
/// l'operatore non e' ordinato: invariante interna violata dal chiamante
/// (`rule_passes` invoca solo con Gt/Ge/Lt/Le), segnalata come errore
/// Internal, non panic.
const fn rule_ordered(ordering: Option<Ordering>, operator: RuleOperator) -> Option<bool> {
    match operator {
        RuleOperator::Gt => Some(matches!(ordering, Some(Ordering::Greater))),
        RuleOperator::Ge => Some(matches!(
            ordering,
            Some(Ordering::Greater | Ordering::Equal)
        )),
        RuleOperator::Lt => Some(matches!(ordering, Some(Ordering::Less))),
        RuleOperator::Le => Some(matches!(ordering, Some(Ordering::Less | Ordering::Equal))),
        _ => None,
    }
}

/// Esito del confronto fra una cella e l'estremo di una regola.
///
/// Un confronto non definito (`Undefined`, un solo lato NaN) segue IEEE: `ne`
/// resta vero. Una cella non interpretabile (`Invalid`: errore di
/// conversione, estremo assente) fa fallire la regola per ogni operatore,
/// `ne` compreso; con un solo `equal = false` `ne` passerebbe proprio sui
/// valori illeggibili.
#[derive(Clone, Copy)]
enum RuleComparison {
    /// Ordine definito fra cella ed estremo.
    Ordered(Ordering),
    /// Cella ed estremo entrambi NaN: uguali per la semantica storica dei
    /// double, ma non ordinati.
    BothNan,
    /// Confronto non definito (un solo lato NaN). La cella resta leggibile.
    Undefined,
    /// Cella o estremo non interpretabili.
    Invalid,
}

impl RuleComparison {
    /// Ordine per gli operatori ordinati; `None` quando il confronto non e'
    /// definito o la cella non e' interpretabile — in entrambi i casi ogni
    /// operatore ordinato e' falso.
    const fn ordering(self) -> Option<Ordering> {
        match self {
            Self::Ordered(ordering) => Some(ordering),
            _ => None,
        }
    }

    /// Uguaglianza per `eq`/`ne`; `None` se la cella non e' interpretabile, e
    /// allora ENTRAMBI gli operatori falliscono.
    const fn equality(self) -> Option<bool> {
        match self {
            Self::Ordered(Ordering::Equal) | Self::BothNan => Some(true),
            Self::Ordered(_) | Self::Undefined => Some(false),
            Self::Invalid => None,
        }
    }
}

/// Confronta la cella con un estremo di regola nel dominio nativo del tipo.
///
/// Int64/UInt64/Float64, Date32, Timestamp(ms) e Decimal128 passano tutti da
/// `scalar_compare`, che confronta senza mai convertire a `f64`: nessun
/// collasso degli interi oltre 2^53, nessun arrotondamento dei decimal.
fn rule_compare(array: &dyn Array, row: usize, bound: Option<NumericBound>) -> RuleComparison {
    let Some(bound) = bound else {
        return RuleComparison::Invalid;
    };
    // Semantica storica dei double (NaN uguale a NaN): precede il comparatore
    // tipizzato, che segue IEEE e non distingue i due casi di NaN.
    if let (Some(values), NumericBound::F64(expected)) =
        (array.as_any().downcast_ref::<Float64Array>(), bound)
    {
        let actual = values.value(row);
        if actual.is_nan() || expected.is_nan() {
            return if actual.is_nan() && expected.is_nan() {
                RuleComparison::BothNan
            } else {
                RuleComparison::Undefined
            };
        }
    }
    match scalar_compare(array, row, bound) {
        Ok(Some(ordering)) => RuleComparison::Ordered(ordering),
        Ok(None) => RuleComparison::Undefined,
        Err(_) => RuleComparison::Invalid,
    }
}

/// Colonna di una regola risolta una volta per regola: il tipo fisico
/// decide null, confronto, testo e regex senza un downcast per riga.
///
/// Ogni ramo tipizzato rende cio' che renderebbe il percorso scalare sulla
/// stessa cella (`is_logically_null`, `rule_compare`, `scalar_as_string`),
/// che resta il ramo `Generica`.
struct ColonnaRegola<'a> {
    array: &'a dyn Array,
    tipo: TipoColonnaRegola<'a>,
}

#[derive(Clone, Copy)]
enum TipoColonnaRegola<'a> {
    Int64(&'a Int64Array),
    Float64(&'a Float64Array),
    Utf8(&'a StringArray),
    Generica,
}

impl<'a> ColonnaRegola<'a> {
    fn new(array: &'a dyn Array) -> Self {
        let any = array.as_any();
        let tipo = any
            .downcast_ref::<Int64Array>()
            .map(TipoColonnaRegola::Int64)
            .or_else(|| {
                any.downcast_ref::<Float64Array>()
                    .map(TipoColonnaRegola::Float64)
            })
            .or_else(|| {
                any.downcast_ref::<StringArray>()
                    .map(TipoColonnaRegola::Utf8)
            })
            .unwrap_or(TipoColonnaRegola::Generica);
        Self { array, tipo }
    }

    /// Null logico: per i tipi nativi e' il null fisico (non sono
    /// dictionary).
    fn nulla(&self, row: usize) -> bool {
        match self.tipo {
            TipoColonnaRegola::Int64(values) => values.is_null(row),
            TipoColonnaRegola::Float64(values) => values.is_null(row),
            TipoColonnaRegola::Utf8(values) => values.is_null(row),
            TipoColonnaRegola::Generica => crate::is_logically_null(self.array, row),
        }
    }

    /// [`rule_compare`] sulla cella: `scalar_compare` su Int64 e Float64 non
    /// fallisce e si riduce a `compare_i64`/`compare_f64`, con davanti la
    /// semantica storica dei NaN dei double.
    fn confronto(&self, row: usize, bound: Option<NumericBound>) -> RuleComparison {
        let ordine = |ordine: Option<Ordering>| {
            ordine.map_or(RuleComparison::Undefined, RuleComparison::Ordered)
        };
        match (self.tipo, bound) {
            (_, None) => RuleComparison::Invalid,
            (TipoColonnaRegola::Int64(values), Some(bound)) => {
                ordine(compare_i64(values.value(row), bound))
            }
            (TipoColonnaRegola::Float64(values), Some(bound)) => {
                let actual = values.value(row);
                if let NumericBound::F64(expected) = bound {
                    if actual.is_nan() || expected.is_nan() {
                        return if actual.is_nan() && expected.is_nan() {
                            RuleComparison::BothNan
                        } else {
                            RuleComparison::Undefined
                        };
                    }
                }
                ordine(compare_f64(actual, bound))
            }
            (TipoColonnaRegola::Utf8(_) | TipoColonnaRegola::Generica, bound) => {
                rule_compare(self.array, row, bound)
            }
        }
    }

    /// Uguaglianza testuale con `expected`; `None` se la cella non si legge.
    fn testo_uguale(&self, row: usize, expected: &str) -> Option<bool> {
        match self.tipo {
            TipoColonnaRegola::Utf8(values) => {
                (!values.is_null(row)).then(|| values.value(row) == expected)
            }
            _ => match scalar_as_string(self.array, row) {
                Ok(Some(actual)) => Some(actual == expected),
                Ok(None) | Err(_) => None,
            },
        }
    }

    /// La regex riconosce la cella; una cella che non si legge non la
    /// soddisfa.
    fn corrisponde(&self, row: usize, regex: &regex::Regex) -> bool {
        match self.tipo {
            TipoColonnaRegola::Utf8(values) => {
                !values.is_null(row) && regex.is_match(values.value(row))
            }
            _ => scalar_as_string(self.array, row)
                .ok()
                .flatten()
                .is_some_and(|actual| regex.is_match(&actual)),
        }
    }
}

/// Valuta una regola su una riga.
///
/// MAI un errore sui dati: qualunque valore non interpretabile (incluso null
/// per gli operatori a valore) e' un fallimento della regola, non un errore
/// del kernel. Il `Result` copre solo invarianti interne violate (errore
/// Internal), mai i dati.
///
/// Confronti numerici tutti esatti nel dominio nativo del tipo (vedi
/// [`rule_compare`]); l'uguaglianza sulle colonne non numeriche resta
/// testuale, come da contratto della regola.
fn rule_passes(rule: &CompiledRule, colonna: &ColonnaRegola<'_>, row: usize) -> Result<bool> {
    match rule.operator {
        // Stessa nozione di null del filtro: due kernel che rispondono
        // diversamente sulla stessa riga sarebbero peggio di entrambi.
        RuleOperator::Isnull => return Ok(colonna.nulla(row)),
        RuleOperator::Notnull => return Ok(!colonna.nulla(row)),
        _ if colonna.nulla(row) => return Ok(false),
        _ => {}
    }
    Ok(match rule.operator {
        RuleOperator::Eq | RuleOperator::Ne => {
            let equal = if rule.numeric_column {
                colonna.confronto(row, rule.expected_bound).equality()
            } else {
                // Colonna non numerica: uguaglianza testuale. Una cella che
                // non si riesce a leggere e' `None` (non interpretabile), mai
                // "diversa dal valore atteso".
                colonna.testo_uguale(row, &rule.expected)
            };
            // `None` = cella non interpretabile: la regola fallisce sia con
            // `eq` sia con `ne`.
            equal.is_some_and(|equal| matches!(rule.operator, RuleOperator::Ne) != equal)
        }
        RuleOperator::Gt | RuleOperator::Ge | RuleOperator::Lt | RuleOperator::Le => rule_ordered(
            colonna.confronto(row, rule.expected_bound).ordering(),
            rule.operator,
        )
        .ok_or_else(|| PlenoraError::Internal("operatore di regola non ordinato".into()))?,
        RuleOperator::Range => within_rule_range(
            colonna.confronto(row, rule.expected_bound).ordering(),
            colonna.confronto(row, rule.expected_high_bound).ordering(),
        ),
        RuleOperator::Regex => rule
            .regex
            .as_ref()
            .is_some_and(|regex| colonna.corrisponde(row, regex)),
        RuleOperator::Isnull | RuleOperator::Notnull => {
            return Err(PlenoraError::Internal(
                "isnull/notnull sono valutati prima del confronto scalare".into(),
            ));
        }
    })
}

/// Aggiunge `nome` all'elenco `;`-separato di una riga.
fn aggiungi_nome(elenco: &mut String, vuoto: &mut bool, nome: &str) {
    if !*vuoto {
        elenco.push(';');
    }
    elenco.push_str(nome);
    *vuoto = false;
}

/// Valida le righe contro un set di regole dichiarative.
///
/// NON fallisce mai sui dati: `annotate` aggiunge `_valid` (Boolean, false
/// se almeno una regola error e' fallita), `_errors` e `_warnings` (nomi
/// delle regole fallite separati da `;`, stringa vuota se nessuna);
/// `summary` emette una riga per regola (`name`, `errors`, `warnings` con i
/// conteggi delle righe fallite per gravita').
///
/// Le regole si valutano riga per riga nell'ordine di configurazione, con
/// la colonna di ogni regola risolta una volta (`ColonnaRegola`); i nomi
/// delle regole fallite si scrivono direttamente nei buffer di output.
///
/// # Errors
///
/// - `InvalidPlan`: nessuna regola; nome regola vuoto o ripetuto; regola senza
///   `column`; `value` mancante o non ammesso per l'operatore; tipo della
///   colonna incompatibile con l'operatore; valore atteso non numerico o
///   range malformato; regex non valida;
/// - `Schema`: colonna di una regola assente;
/// - `Internal`: invarianti interne violate;
/// - `DataMapping` (`arrow error`): errore Arrow nella costruzione
///   dell'output.
pub fn validate_rules(batch: &RecordBatch, config: &ValidateRules) -> Result<RecordBatch> {
    let rules = compile_rules(batch, config)?;
    let colonne = rules
        .iter()
        .map(|rule| ColonnaRegola::new(batch.column(rule.column_index).as_ref()))
        .collect::<Vec<_>>();
    let righe = batch.num_rows();
    match config.output_mode {
        ValidateOutputMode::Annotate => {
            let mut valid = Vec::with_capacity(righe);
            let mut errors = StringBuilder::with_capacity(righe, 0);
            let mut warnings = StringBuilder::with_capacity(righe, 0);
            let mut testo_errori = String::new();
            let mut testo_avvisi = String::new();
            for row in 0..righe {
                testo_errori.clear();
                testo_avvisi.clear();
                let mut senza_errori = true;
                let mut senza_avvisi = true;
                for (rule, colonna) in rules.iter().zip(&colonne) {
                    if !rule_passes(rule, colonna, row)? {
                        match rule.severity {
                            RuleSeverity::Error => {
                                aggiungi_nome(&mut testo_errori, &mut senza_errori, &rule.name);
                            }
                            RuleSeverity::Warning => {
                                aggiungi_nome(&mut testo_avvisi, &mut senza_avvisi, &rule.name);
                            }
                        }
                    }
                }
                valid.push(senza_errori);
                errors.append_value(&testo_errori);
                warnings.append_value(&testo_avvisi);
            }
            let result = replace_or_append(
                batch,
                "_valid",
                DataType::Boolean,
                false,
                Arc::new(BooleanArray::from(valid)),
            )?;
            let result = replace_or_append(
                &result,
                "_errors",
                DataType::Utf8,
                false,
                Arc::new(errors.finish()),
            )?;
            replace_or_append(
                &result,
                "_warnings",
                DataType::Utf8,
                false,
                Arc::new(warnings.finish()),
            )
        }
        ValidateOutputMode::Summary => {
            let mut error_counts = vec![0_i64; rules.len()];
            let mut warning_counts = vec![0_i64; rules.len()];
            for row in 0..righe {
                for ((rule, colonna), (errori, avvisi)) in rules
                    .iter()
                    .zip(&colonne)
                    .zip(error_counts.iter_mut().zip(warning_counts.iter_mut()))
                {
                    if !rule_passes(rule, colonna, row)? {
                        match rule.severity {
                            RuleSeverity::Error => *errori += 1,
                            RuleSeverity::Warning => *avvisi += 1,
                        }
                    }
                }
            }
            Ok(RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("name", DataType::Utf8, false),
                    Field::new("errors", DataType::Int64, false),
                    Field::new("warnings", DataType::Int64, false),
                ])),
                vec![
                    Arc::new(StringArray::from(
                        rules
                            .iter()
                            .map(|rule| rule.name.as_str())
                            .collect::<Vec<_>>(),
                    )),
                    Arc::new(Int64Array::from(error_counts)),
                    Arc::new(Int64Array::from(warning_counts)),
                ],
            )?)
        }
    }
}

#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // Test-oracolo di `reconcile`/`assert_foreign_key`: le implementazioni
    // di riferimento stanno qui sotto, indipendenti dal percorso
    // ottimizzato, e i byte delle chiavi dell'encoder sono confrontati
    // direttamente con `quality::key_for_row`.
    // -------------------------------------------------------------------

    use super::*;
    use crate::quality::key_for_row;
    use crate::test_support::batch_from_fields as batch_of;
    use plenora_core::arrow::array::{
        BinaryArray, Date32Array, Decimal128Array, TimestampMillisecondArray,
    };

    /// Oracolo indipendente di `frequencies`, usato da
    /// `reconcile_reference`.
    fn frequencies_reference(
        batch: &RecordBatch,
        indices: &[usize],
        nulls_equal: bool,
        side_nulls: &mut usize,
        memory_used: &mut usize,
        limits: &Limits,
    ) -> Result<HashMap<Vec<u8>, usize>> {
        let mut output: HashMap<Vec<u8>, usize> = HashMap::new();
        for row in 0..batch.num_rows() {
            if !nulls_equal && has_null(batch, indices, row) {
                *side_nulls = side_nulls.checked_add(1).ok_or_else(|| {
                    PlenoraError::ResourceLimit("overflow null reconciliation".into())
                })?;
                continue;
            }
            let key = key_for_row(batch, indices, row)?;
            if let Some(count) = output.get_mut(&key) {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| PlenoraError::ResourceLimit("overflow reconciliation".into()))?;
            } else {
                *memory_used = memory_used
                    .checked_add(key.len().saturating_add(64))
                    .ok_or_else(|| {
                        PlenoraError::ResourceLimit("overflow memoria reconciliation".into())
                    })?;
                if *memory_used > limits.max_governed_memory_bytes {
                    return Err(PlenoraError::ResourceLimit(
                        "reconcile oltre max_governed_memory_bytes".into(),
                    ));
                }
                output.insert(key, 1);
                if output.len() > limits.max_rows {
                    return Err(PlenoraError::ResourceLimit(
                        "reconcile supera max_rows chiavi distinte".into(),
                    ));
                }
            }
        }
        Ok(output)
    }

    /// Oracolo indipendente di `reconcile`: stesso contratto, percorso
    /// diverso.
    fn reconcile_reference(
        left: &RecordBatch,
        right: &RecordBatch,
        config: &Reconcile,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let left_indices = key_indices(left, &config.left_keys)?;
        let right_indices = key_indices(right, &config.right_keys)?;
        validate_key_types(left, right, &left_indices, &right_indices)?;
        let mut left_nulls = 0;
        let mut right_nulls = 0;
        let mut memory_used = 0_usize;
        let left_counts = frequencies_reference(
            left,
            &left_indices,
            config.nulls_equal,
            &mut left_nulls,
            &mut memory_used,
            limits,
        )?;
        let right_counts = frequencies_reference(
            right,
            &right_indices,
            config.nulls_equal,
            &mut right_nulls,
            &mut memory_used,
            limits,
        )?;
        let mut matched = 0_usize;
        let mut left_only = left_nulls;
        let mut right_only = right_nulls;
        let mut left_duplicates = 0_usize;
        let mut right_duplicates = 0_usize;
        for (key, left_count) in &left_counts {
            let right_count = right_counts.get(key).copied().unwrap_or_default();
            let common = (*left_count).min(right_count);
            matched = matched.saturating_add(common);
            left_only = left_only.saturating_add(left_count - common);
            left_duplicates = left_duplicates.saturating_add(left_count.saturating_sub(1));
        }
        for (key, right_count) in &right_counts {
            let left_count = left_counts.get(key).copied().unwrap_or_default();
            right_only = right_only.saturating_add(right_count.saturating_sub(left_count));
            right_duplicates = right_duplicates.saturating_add(right_count.saturating_sub(1));
        }
        let metrics = [
            "matched_rows",
            "left_only_rows",
            "right_only_rows",
            "left_duplicate_rows",
            "right_duplicate_rows",
        ];
        let values = [
            matched,
            left_only,
            right_only,
            left_duplicates,
            right_duplicates,
        ]
        .into_iter()
        .map(as_u64)
        .collect::<Result<Vec<_>>>()?;
        Ok(RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::UInt64, false),
            ])),
            vec![
                Arc::new(StringArray::from(metrics.to_vec())),
                Arc::new(UInt64Array::from(values)),
            ],
        )?)
    }

    /// Oracolo indipendente di `assert_foreign_key`: stesso contratto,
    /// percorso diverso.
    fn assert_foreign_key_reference(
        left: &RecordBatch,
        right: &RecordBatch,
        config: &ForeignKey,
        limits: &Limits,
    ) -> Result<RecordBatch> {
        let left_indices = key_indices(left, &config.left_keys)?;
        let right_indices = key_indices(right, &config.right_keys)?;
        validate_key_types(left, right, &left_indices, &right_indices)?;
        let mut referenced = HashSet::with_capacity(right.num_rows());
        let mut memory_used = 0_usize;
        for row in 0..right.num_rows() {
            if !has_null(right, &right_indices, row) {
                let key = key_for_row(right, &right_indices, row)?;
                let key_bytes = key.len();
                if referenced.insert(key) {
                    memory_used = memory_used
                        .checked_add(key_bytes.saturating_add(64))
                        .ok_or_else(|| {
                            PlenoraError::ResourceLimit("overflow memoria foreign key".into())
                        })?;
                    if memory_used > limits.max_governed_memory_bytes {
                        return Err(PlenoraError::ResourceLimit(
                            "assert_foreign_key oltre max_governed_memory_bytes".into(),
                        ));
                    }
                }
            }
        }
        for row in 0..left.num_rows() {
            if has_null(left, &left_indices, row) {
                if config.allow_null {
                    continue;
                }
                return Err(PlenoraError::InvalidPlan(format!(
                    "assert_foreign_key: chiave null alla riga {row}"
                )));
            }
            if !referenced.contains(&key_for_row(left, &left_indices, row)?) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "assert_foreign_key: riferimento mancante alla riga {row}"
                )));
            }
        }
        Ok(left.clone())
    }

    /// Fixture con chiavi composite su tipi misti (int64 + utf8 nullable)
    /// con duplicati e null.
    fn mixed_batch(ids: Vec<Option<i64>>, tags: Vec<Option<&str>>) -> RecordBatch {
        batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("tag", DataType::Utf8, true),
            ],
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(tags)),
            ],
        )
    }

    /// Confronto rigoroso dell'output di `reconcile`: schema, righe, valori.
    fn assert_reconcile_identical(fast: &RecordBatch, reference: &RecordBatch) {
        assert_eq!(fast.num_rows(), reference.num_rows(), "righe");
        assert_eq!(fast.num_columns(), reference.num_columns(), "colonne");
        let metrics_fast = fast
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("metric fast");
        let metrics_ref = reference
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("metric ref");
        let values_fast = fast
            .column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("value fast");
        let values_ref = reference
            .column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("value ref");
        for row in 0..fast.num_rows() {
            assert_eq!(
                metrics_fast.value(row),
                metrics_ref.value(row),
                "metrica {row}"
            );
            assert_eq!(
                values_fast.value(row),
                values_ref.value(row),
                "valore {row}"
            );
        }
    }

    #[test]
    fn row_key_encoder_bytes_match_key_for_row_on_all_types() {
        let timestamp = TimestampMillisecondArray::from(vec![
            Some(0),
            None,
            Some(-1_000),
            Some(1_700_000_000_000),
        ]);
        let decimal = Decimal128Array::from(vec![Some(12_345), None, Some(-1), Some(0)])
            .with_precision_and_scale(38, 2)
            .expect("decimal");
        let batch = batch_of(
            vec![
                Field::new("i", DataType::Int64, true),
                Field::new("s", DataType::Utf8, true),
                Field::new("f", DataType::Float64, true),
                Field::new("b", DataType::Boolean, true),
                Field::new("u", DataType::UInt64, true),
                Field::new("d", DataType::Date32, true),
                Field::new("t", timestamp.data_type().clone(), true),
                Field::new("m", decimal.data_type().clone(), true),
                Field::new("x", DataType::Binary, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![
                    Some(1),
                    None,
                    Some(-5),
                    Some(i64::MAX),
                ])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    None,
                    Some("héllo"),
                    Some(""),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(f64::NAN),
                    Some(-0.0),
                    Some(f64::INFINITY),
                    Some(1.5),
                ])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    None,
                    Some(false),
                    Some(true),
                ])),
                Arc::new(UInt64Array::from(vec![
                    Some(0),
                    None,
                    Some(u64::MAX),
                    Some(7),
                ])),
                Arc::new(Date32Array::from(vec![
                    Some(0),
                    None,
                    Some(-1),
                    Some(19_000),
                ])),
                Arc::new(timestamp),
                Arc::new(decimal),
                Arc::new(BinaryArray::from(vec![
                    Some(&b"ab"[..]),
                    None,
                    Some(&b""[..]),
                    Some(&b"c"[..]),
                ])),
            ],
        );
        let indices: Vec<usize> = (0..batch.num_columns()).collect();
        let mut encoder = RowKeyEncoder::new(&batch, &indices);
        let mut key = Vec::new();
        for row in 0..batch.num_rows() {
            encoder.encode_into(row, &mut key).expect("encode");
            let reference = key_for_row(&batch, &indices, row).expect("key_for_row");
            assert_eq!(key, reference, "byte chiave riga {row}");
        }
    }

    #[test]
    fn reconcile_matches_reference_with_duplicates_and_nulls() {
        let left = mixed_batch(
            vec![Some(1), Some(2), Some(2), None, Some(3), Some(3), Some(3)],
            vec![Some("a"), Some("b"), Some("b"), Some("n"), None, None, None],
        );
        let right = mixed_batch(
            vec![Some(2), Some(2), Some(9), None, Some(3), Some(3)],
            vec![Some("b"), Some("b"), Some("z"), Some("n"), None, Some("x")],
        );
        for nulls_equal in [true, false] {
            let config = Reconcile {
                left_keys: vec!["id".into(), "tag".into()],
                right_keys: vec!["id".into(), "tag".into()],
                nulls_equal,
            };
            let fast = reconcile(&left, &right, &config, &Limits::default()).expect("fast");
            let reference =
                reconcile_reference(&left, &right, &config, &Limits::default()).expect("reference");
            assert_reconcile_identical(&fast, &reference);
        }
    }

    #[test]
    fn reconcile_matches_reference_on_empty_inputs() {
        let left = mixed_batch(Vec::new(), Vec::new());
        let right = mixed_batch(Vec::new(), Vec::new());
        let config = Reconcile {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            nulls_equal: true,
        };
        let fast = reconcile(&left, &right, &config, &Limits::default()).expect("fast");
        let reference =
            reconcile_reference(&left, &right, &config, &Limits::default()).expect("reference");
        assert_reconcile_identical(&fast, &reference);
    }

    #[test]
    fn reconcile_errors_match_reference() {
        let left = mixed_batch(
            vec![Some(1), Some(2), Some(3)],
            vec![Some("a"), Some("b"), Some("c")],
        );
        let right = mixed_batch(vec![Some(1)], vec![Some("a")]);
        let config = Reconcile {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            nulls_equal: true,
        };
        // max_governed_memory_bytes esaurito: stesso errore nelle due versioni.
        let tight_memory = Limits {
            max_governed_memory_bytes: 4,
            ..Limits::default()
        };
        let fast = reconcile(&left, &right, &config, &tight_memory).expect_err("fast err");
        let reference =
            reconcile_reference(&left, &right, &config, &tight_memory).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore memoria");
        // max_rows sulle chiavi distinte: stesso errore.
        let tight_rows = Limits {
            max_rows: 2,
            ..Limits::default()
        };
        let fast = reconcile(&left, &right, &config, &tight_rows).expect_err("fast err");
        let reference =
            reconcile_reference(&left, &right, &config, &tight_rows).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore max_rows");
        // Tipi chiave diversi: stesso errore di schema.
        let wrong = batch_of(
            vec![Field::new("id", DataType::Utf8, true)],
            vec![Arc::new(StringArray::from(vec![Some("1")]))],
        );
        let fast = reconcile(&left, &wrong, &config, &Limits::default()).expect_err("fast err");
        let reference =
            reconcile_reference(&left, &wrong, &config, &Limits::default()).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore tipi");
    }

    #[test]
    fn assert_foreign_key_matches_reference() {
        let right = mixed_batch(
            vec![Some(1), Some(2), Some(2), None, Some(3)],
            vec![Some("a"), Some("b"), Some("b"), Some("n"), Some("c")],
        );
        let left = mixed_batch(
            vec![Some(2), Some(1), Some(3), Some(2)],
            vec![Some("b"), Some("a"), Some("c"), Some("b")],
        );
        let config = ForeignKey {
            left_keys: vec!["id".into(), "tag".into()],
            right_keys: vec!["id".into(), "tag".into()],
            allow_null: false,
        };
        let fast = assert_foreign_key(&left, &right, &config, &Limits::default()).expect("fast");
        let reference = assert_foreign_key_reference(&left, &right, &config, &Limits::default())
            .expect("reference");
        assert_eq!(fast.num_rows(), reference.num_rows());
        assert_eq!(fast.schema(), reference.schema());
    }

    #[test]
    fn assert_foreign_key_errors_are_row_scoped_and_non_row_errors_match_reference() {
        let right = mixed_batch(vec![Some(1), Some(2)], vec![Some("a"), Some("b")]);
        // Riferimento mancante: stessa riga, ma diagnostica strutturata e
        // senza valore della chiave.
        let left = mixed_batch(vec![Some(1), Some(9)], vec![Some("a"), Some("z")]);
        let config = ForeignKey {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            allow_null: false,
        };
        let fast =
            assert_foreign_key(&left, &right, &config, &Limits::default()).expect_err("fast err");
        let report = fast.row_diagnostics().expect("diagnostica foreign key");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.total, Some(1));
        assert_eq!(report.examples[0].source_index, 1);
        assert_eq!(report.examples[0].cause, "validation.foreign_key_missing");
        // Chiave null con allow_null=false: diagnostica distinta.
        let left_null = mixed_batch(vec![Some(1), None], vec![Some("a"), Some("x")]);
        let fast = assert_foreign_key(&left_null, &right, &config, &Limits::default())
            .expect_err("fast err");
        let report = fast.row_diagnostics().expect("diagnostica chiave null");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.examples[0].source_index, 1);
        assert_eq!(report.examples[0].cause, "validation.foreign_key_null");
        // allow_null=true: le null vengono saltate in entrambe.
        let config_allow = ForeignKey {
            left_keys: vec!["id".into()],
            right_keys: vec!["id".into()],
            allow_null: true,
        };
        assert!(assert_foreign_key(&left_null, &right, &config_allow, &Limits::default()).is_ok());
        assert!(assert_foreign_key_reference(
            &left_null,
            &right,
            &config_allow,
            &Limits::default()
        )
        .is_ok());
        // max_governed_memory_bytes: stesso errore (contabilita' basata sui byte chiave).
        let tight = Limits {
            max_governed_memory_bytes: 4,
            ..Limits::default()
        };
        let fast = assert_foreign_key(&left, &right, &config, &tight).expect_err("fast err");
        let reference =
            assert_foreign_key_reference(&left, &right, &config, &tight).expect_err("ref err");
        assert_eq!(fast.to_string(), reference.to_string(), "errore memoria");
    }

    // -------------------------------------------------------------------
    // table.validate_rules
    // -------------------------------------------------------------------

    fn rules_batch() -> RecordBatch {
        batch_of(
            vec![
                Field::new("id", DataType::Int64, true),
                Field::new("name", DataType::Utf8, true),
                Field::new("score", DataType::Float64, true),
            ],
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(-2), None, Some(4)])),
                Arc::new(StringArray::from(vec![
                    Some("alfa"),
                    Some("beta"),
                    Some(""),
                    None,
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(0.5),
                    Some(9.9),
                    Some(f64::NAN),
                    Some(-1.0),
                ])),
            ],
        )
    }

    fn rules(config: serde_json::Value) -> ValidateRules {
        serde_json::from_value(config).expect("config validate_rules")
    }

    fn annotated_strings(output: &RecordBatch, name: &str) -> Vec<String> {
        let column = output
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .expect("utf8");
        (0..column.len())
            .map(|row| column.value(row).to_owned())
            .collect()
    }

    #[test]
    fn validate_rules_annotate_never_fails_on_data() {
        let output = validate_rules(
            &rules_batch(),
            &rules(serde_json::json!({
                "rules": [
                    {"name": "id_pos", "operator": "gt", "column": "id", "value": 0},
                    {"name": "name_fmt", "operator": "regex", "column": "name", "value": "^[a-z]+$", "severity": "warning"},
                    {"name": "score_range", "operator": "range", "column": "score", "value": "0,10"},
                    {"name": "name_present", "operator": "notnull", "column": "name"}
                ]
            })),
        )
        .expect("annotate");
        let valid = output
            .column_by_name("_valid")
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .expect("_valid");
        let flags: Vec<bool> = (0..4).map(|row| valid.value(row)).collect();
        // riga 0: tutto ok; riga 1: id -2 fallisce; riga 2: id null fallisce
        // (NaN nello score fallisce il range); riga 3: name null -> warning +
        // notnull fallita, score -1 fuori range -> errore.
        assert_eq!(flags, [true, false, false, false]);
        assert_eq!(
            annotated_strings(&output, "_errors"),
            [
                "",
                "id_pos",
                "id_pos;score_range",
                "score_range;name_present"
            ]
        );
        assert_eq!(
            annotated_strings(&output, "_warnings"),
            ["", "", "name_fmt", "name_fmt"]
        );
        // Colonne non nullable, schema input preservato.
        let schema = output.schema();
        for name in ["_valid", "_errors", "_warnings"] {
            assert!(!schema.field_with_name(name).expect(name).is_nullable());
        }
        assert!(schema.field_with_name("id").is_ok());
    }

    #[test]
    fn validate_rules_summary_counts_per_rule() {
        let output = validate_rules(
            &rules_batch(),
            &rules(serde_json::json!({
                "output_mode": "summary",
                "rules": [
                    {"name": "id_pos", "operator": "gt", "column": "id", "value": 0},
                    {"name": "name_fmt", "operator": "regex", "column": "name", "value": "^[a-z]+$", "severity": "warning"},
                    {"name": "name_missing", "operator": "isnull", "column": "name", "severity": "warning"}
                ]
            })),
        )
        .expect("summary");
        assert_eq!(output.num_rows(), 3);
        assert_eq!(
            annotated_strings(&output, "name"),
            ["id_pos", "name_fmt", "name_missing"]
        );
        let errors = output
            .column_by_name("errors")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("errors");
        let warnings = output
            .column_by_name("warnings")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .expect("warnings");
        assert_eq!(
            (0..3).map(|r| errors.value(r)).collect::<Vec<_>>(),
            [2, 0, 0]
        );
        assert_eq!(
            (0..3).map(|r| warnings.value(r)).collect::<Vec<_>>(),
            [0, 2, 3]
        );
    }

    #[test]
    fn validate_rules_static_errors_before_data() {
        let batch = rules_batch();
        // Regex invalida: errore di validazione, non a meta' dei dati.
        let error = validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "regex", "column": "name", "value": "("}]
            })),
        )
        .expect_err("regex invalida");
        assert!(error.to_string().contains("regex non valida"));
        // Confronto ordinato su colonna non numerica: errore di validazione.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "gt", "column": "name", "value": 1}]
            })),
        )
        .is_err());
        // Regex su colonna non Utf8.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "regex", "column": "id", "value": "1"}]
            })),
        )
        .is_err());
        // Eq numerica con valore non numerico; range malformato.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "eq", "column": "id", "value": "abc"}]
            })),
        )
        .is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "range", "column": "id", "value": "1-2"}]
            })),
        )
        .is_err());
        // value mancante / non ammesso; colonna mancante; regole vuote;
        // nomi duplicati.
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "eq", "column": "id"}]
            })),
        )
        .is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "isnull", "column": "id", "value": 1}]
            })),
        )
        .is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [{"name": "bad", "operator": "eq", "column": "missing", "value": 1}]
            })),
        )
        .is_err());
        assert!(validate_rules(&batch, &rules(serde_json::json!({"rules": []}))).is_err());
        assert!(validate_rules(
            &batch,
            &rules(serde_json::json!({
                "rules": [
                    {"name": "dup", "operator": "isnull", "column": "id"},
                    {"name": "dup", "operator": "notnull", "column": "id"}
                ]
            })),
        )
        .is_err());
        // Config strict: campo sconosciuto rifiutato.
        assert!(serde_json::from_value::<ValidateRules>(serde_json::json!({
            "rules": [{"name": "r", "operator": "eq", "column": "id", "value": 1, "surprise": 1}]
        }))
        .is_err());
    }

    #[test]
    fn validate_rules_eq_ne_and_empty_batch() {
        let output = validate_rules(
            &rules_batch(),
            &rules(serde_json::json!({
                "rules": [
                    {"name": "is_alfa", "operator": "eq", "column": "name", "value": "alfa"},
                    {"name": "not_beta", "operator": "ne", "column": "name", "value": "beta"}
                ]
            })),
        )
        .expect("eq/ne");
        assert_eq!(
            annotated_strings(&output, "_errors"),
            ["", "is_alfa;not_beta", "is_alfa", "is_alfa;not_beta"]
        );
        // Batch vuoto: annotate con zero righe, summary con i conteggi a zero.
        let empty = batch_of(
            vec![Field::new("id", DataType::Int64, true)],
            vec![Arc::new(Int64Array::from(Vec::<Option<i64>>::new()))],
        );
        let config = rules(serde_json::json!({
            "rules": [{"name": "r", "operator": "gt", "column": "id", "value": 0}]
        }));
        let annotated = validate_rules(&empty, &config).expect("empty annotate");
        assert_eq!(annotated.num_rows(), 0);
        assert_eq!(annotated.num_columns(), 4);
        let summary = validate_rules(
            &empty,
            &rules(serde_json::json!({
                "output_mode": "summary",
                "rules": [{"name": "r", "operator": "gt", "column": "id", "value": 0}]
            })),
        )
        .expect("empty summary");
        assert_eq!(summary.num_rows(), 1);
    }

    #[test]
    fn validate_rules_integer_comparisons_are_exact_beyond_2_pow_53() {
        // Classe "confronti via f64": u64::MAX-1 e u64::MAX (come 2^53 e
        // 2^53+1) collassano sullo stesso double; eq/ordinati/range devono
        // restare esatti.
        let uints = batch_of(
            vec![Field::new("u", DataType::UInt64, true)],
            vec![Arc::new(UInt64Array::from(vec![
                Some(u64::MAX - 1),
                Some(u64::MAX),
                Some(9),
                None,
            ]))],
        );
        let output = validate_rules(
            &uints,
            &rules(serde_json::json!({
                "rules": [
                    {"name": "is_max", "operator": "eq", "column": "u", "value": "18446744073709551615"},
                    {"name": "top_range", "operator": "range", "column": "u", "value": "18446744073709551615,18446744073709551615"},
                    {"name": "gt_max_minus_one", "operator": "gt", "column": "u", "value": "18446744073709551614"}
                ]
            })),
        )
        .expect("u64 esatti");
        // riga 0 (MAX-1): tutte falliscono; riga 1 (MAX): tutte passano;
        // riga 2 (9): tutte falliscono; riga 3 (null): tutte falliscono.
        assert_eq!(
            annotated_strings(&output, "_errors"),
            [
                "is_max;top_range;gt_max_minus_one",
                "",
                "is_max;top_range;gt_max_minus_one",
                "is_max;top_range;gt_max_minus_one"
            ]
        );

        let ints = batch_of(
            vec![Field::new("i", DataType::Int64, true)],
            vec![Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_992), // 2^53
                Some(9_007_199_254_740_993), // 2^53 + 1
            ]))],
        );
        let output = validate_rules(
            &ints,
            &rules(serde_json::json!({
                "rules": [
                    {"name": "is_hi", "operator": "eq", "column": "i", "value": 9_007_199_254_740_993_i64},
                    {"name": "hi_range", "operator": "range", "column": "i", "value": "9007199254740993,9007199254740993"},
                    {"name": "ge_lo_double", "operator": "ge", "column": "i", "value": 9_007_199_254_740_992.0_f64}
                ]
            })),
        )
        .expect("i64 esatti");
        // riga 0 (2^53): eq/range falliscono, ge passa (uguale al double);
        // riga 1 (2^53+1): tutte passano (maggiore del double 2^53).
        assert_eq!(
            annotated_strings(&output, "_errors"),
            ["is_hi;hi_range", ""]
        );
    }
}

#[cfg(test)]
#[path = "regole_oracolo.rs"]
mod regole_oracolo;

#[cfg(test)]
#[path = "chiavi_riga_oracolo.rs"]
mod chiavi_riga_oracolo;
