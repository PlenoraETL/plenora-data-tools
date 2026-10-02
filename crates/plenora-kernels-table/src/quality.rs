//! Asserzioni di qualita' sui dati (`table.assert_schema`,
//! `table.assert_not_null`, `table.assert_unique`, `table.assert_range`,
//! `table.assert_regex`) e `table.coalesce`.
//!
//! Un'asserzione che regge restituisce il batch invariato; una violazione
//! e' un errore, mai un batch ridotto. Le violazioni per riga escono come
//! `DataMapping` con diagnostica per riga (`reject_rows`): conteggi per
//! causa ed esempi con indice di riga e colonna, mai valori.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use plenora_core::arrow::array::{
    Array, ArrayRef, Float64Array, RecordBatch, StringArray, UInt64Array,
};
use plenora_core::arrow::schema::DataType;
use regex::Regex;
use serde::Deserialize;

use crate::aggregation::visit_key_ids_where;
use crate::{
    column_index, reject_rows, replace_or_append, scalar_as_string, scalar_compare,
    validate_output_name, NumericBound, NumeroConfig, RowRejection,
};
use plenora_core::{PlenoraError, Result};

/// Una colonna attesa da [`assert_schema`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaExpectation {
    /// Nome della colonna attesa (obbligatorio).
    pub name: String,
    /// Famiglia di tipo attesa (obbligatoria), senza distinzione fra
    /// maiuscole e minuscole e con gli spazi ai lati ignorati: `utf8` o
    /// `string`, `int64` o `integer`, `float64`, `float` o `double`,
    /// `boolean` o `bool`, `uint64` o `unsigned`, `date32`,
    /// `timestamp_seconds`, `timestamp_millis`, `timestamp_micros`,
    /// `timestamp_nanos` (l'unita' conta, il fuso no), `decimal128` (qualunque
    /// precisione e scala), `binary`, `dictionary_utf8` (chiavi `Int32`),
    /// `list` (qualunque elemento), `struct` (qualunque campo).
    pub data_type: String,
    /// Nullabilita' attesa; assente (default), non si controlla.
    pub nullable: Option<bool>,
}

/// Config di `table.assert_schema`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertSchema {
    /// Colonne attese (obbligatorio). L'analisi del contratto rifiuta la
    /// lista vuota, i nomi ripetuti e oltre `max_columns` voci.
    pub fields: Vec<SchemaExpectation>,
    /// Con `false` (default) il batch ha esattamente tante colonne quante
    /// voci in `fields`; con `true` puo' averne altre.
    #[serde(default)]
    pub allow_extra: bool,
    /// Con `true` (default) la voce *i* descrive la colonna in posizione
    /// *i*; con `false` la colonna si cerca per nome.
    #[serde(default = "default_true")]
    pub ordered: bool,
}

const fn default_true() -> bool {
    true
}

fn expected_type(value: &str) -> Result<DataType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "utf8" | "string" => Ok(DataType::Utf8),
        "int64" | "integer" => Ok(DataType::Int64),
        "float64" | "float" | "double" => Ok(DataType::Float64),
        "boolean" | "bool" => Ok(DataType::Boolean),
        "uint64" | "unsigned" => Ok(DataType::UInt64),
        "date32" => Ok(DataType::Date32),
        "timestamp_seconds" => Ok(DataType::Timestamp(
            plenora_core::arrow::schema::TimeUnit::Second,
            None,
        )),
        "timestamp_millis" => Ok(DataType::Timestamp(
            plenora_core::arrow::schema::TimeUnit::Millisecond,
            None,
        )),
        "timestamp_micros" => Ok(DataType::Timestamp(
            plenora_core::arrow::schema::TimeUnit::Microsecond,
            None,
        )),
        "timestamp_nanos" => Ok(DataType::Timestamp(
            plenora_core::arrow::schema::TimeUnit::Nanosecond,
            None,
        )),
        "decimal128" => Ok(DataType::Decimal128(38, 0)),
        "binary" => Ok(DataType::Binary),
        "dictionary_utf8" => Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        )),
        "list" => Ok(DataType::List(Arc::new(
            plenora_core::arrow::schema::Field::new("item", DataType::Null, true),
        ))),
        "struct" => Ok(DataType::Struct(
            plenora_core::arrow::schema::Fields::empty(),
        )),
        other => Err(PlenoraError::InvalidPlan(format!(
            "assert_schema: tipo non supportato {other}"
        ))),
    }
}

fn type_matches(actual: &DataType, expected: &DataType) -> bool {
    match expected {
        DataType::List(_) => matches!(actual, DataType::List(_)),
        DataType::Struct(_) => matches!(actual, DataType::Struct(_)),
        // L'unita' conta, il fuso no: `timestamp_micros` accetta ogni
        // `Timestamp(Microsecond, _)`.
        DataType::Timestamp(unita, None) => {
            matches!(actual, DataType::Timestamp(effettiva, _) if effettiva == unita)
        }
        DataType::Decimal128(_, _) => matches!(actual, DataType::Decimal128(_, _)),
        _ => actual == expected,
    }
}

/// Verifica che lo schema del batch (nomi, tipi, nullabilita') corrisponda
/// alle attese di configurazione; restituisce il batch invariato.
///
/// Con `ordered=true` i campi sono confrontati in posizione, altrimenti per
/// nome; con `allow_extra=false` anche il numero di colonne deve coincidere.
/// Guarda solo lo schema, mai i valori: nel runner l'analisi del contratto
/// rifiuta gli stessi casi in validazione, con `InvalidPlan`.
///
/// # Errors
///
/// - `Schema`: numero di colonne diverso con `allow_extra=false`, colonna
///   assente, nome diverso in posizione (`ordered=true`), tipo o
///   nullabilita' diversi dall'atteso;
/// - `InvalidPlan`: tipo atteso non supportato da `assert_schema`.
pub fn assert_schema(batch: &RecordBatch, config: &AssertSchema) -> Result<RecordBatch> {
    if !config.allow_extra && batch.num_columns() != config.fields.len() {
        return Err(PlenoraError::Schema(format!(
            "assert_schema: attese {} colonne, trovate {}",
            config.fields.len(),
            batch.num_columns()
        )));
    }
    for (position, expectation) in config.fields.iter().enumerate() {
        let index = if config.ordered {
            position
        } else {
            column_index(batch, &expectation.name)?
        };
        let field = batch.schema().fields().get(index).cloned().ok_or_else(|| {
            PlenoraError::Schema(format!(
                "assert_schema: colonna mancante {}",
                expectation.name
            ))
        })?;
        if field.name() != &expectation.name {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: attesa {} in posizione {position}, trovata {}",
                expectation.name,
                field.name()
            )));
        }
        let expected = expected_type(&expectation.data_type)?;
        if !type_matches(field.data_type(), &expected) {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: tipo errato per {}: atteso {}, trovato {}",
                expectation.name,
                expectation.data_type,
                plenora_core::tipo_arrow::descrivi_tipo(field.data_type())
            )));
        }
        if expectation
            .nullable
            .is_some_and(|nullable| nullable != field.is_nullable())
        {
            return Err(PlenoraError::Schema(format!(
                "assert_schema: nullability errata per {}",
                expectation.name
            )));
        }
    }
    Ok(batch.clone())
}

/// Config di `table.assert_not_null`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertNotNull {
    /// Colonne che non devono contenere null (obbligatorio, di qualunque
    /// tipo). L'analisi del contratto rifiuta la lista vuota e i nomi
    /// ripetuti.
    pub columns: Vec<String>,
}

/// Verifica che le colonne configurate non contengano null logici (anche la
/// voce nulla di un dizionario); restituisce il batch invariato.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`);
/// - `DataMapping` con diagnostica per riga: righe con un null in una delle
///   colonne, causa `validation.required_value_missing`; ogni riga conta una
///   volta, con la prima colonna di `columns` in cui e' nulla.
pub fn assert_not_null(batch: &RecordBatch, config: &AssertNotNull) -> Result<RecordBatch> {
    let mut rejections = Vec::new();
    for name in &config.columns {
        let index = column_index(batch, name)?;
        // Null LOGICO: per una dictionary una chiave valida puo' puntare a
        // una entry nulla, e `is_null` non la vedrebbe — `assert_not_null`
        // dichiarerebbe conforme una riga senza valore.
        for row in (0..batch.num_rows())
            .filter(|row| crate::is_logically_null(batch.column(index).as_ref(), *row))
        {
            rejections.push(RowRejection {
                row,
                cause: "validation.required_value_missing",
                column: Some(name),
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

/// Chiave binaria della riga `row` sulle colonne `indices`: prefisso di
/// tipo piu' marcatore di null e valore testuale con lunghezza.
///
/// La codifica e' iniettiva per colonna: chiavi uguali equivalgono a valori
/// uguali secondo il profilo scalare testuale di `scalar_as_string`.
///
/// # Errors
///
/// Come `scalar_as_string`: guardia interna date32 (`InvalidPlan`) oppure
/// valore o tipo non codificabile (`Schema`), compreso un `Binary` non
/// UTF-8.
pub fn key_for_row(batch: &RecordBatch, indices: &[usize], row: usize) -> Result<Vec<u8>> {
    let mut key = Vec::new();
    for index in indices {
        let column = batch.column(*index);
        let type_name = column.data_type().to_string();
        let type_len = type_name.len() as u64;
        key.extend_from_slice(&type_len.to_be_bytes());
        key.extend_from_slice(type_name.as_bytes());
        match crate::scalar_key_string(column.as_ref(), row)? {
            Some(value) => {
                key.push(1);
                let value_len = value.len() as u64;
                key.extend_from_slice(&value_len.to_be_bytes());
                key.extend_from_slice(value.as_bytes());
            }
            None => key.push(0),
        }
    }
    Ok(key)
}

/// Config di `table.assert_unique`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertUnique {
    /// Colonne della chiave (obbligatorio), leggibili come testo scalare.
    /// L'analisi del contratto rifiuta la lista vuota e i nomi ripetuti.
    pub columns: Vec<String>,
    /// Con `true` (default) il null e' un valore della chiave e due null
    /// sono uguali; con `false` le righe con un null logico in una colonna
    /// della chiave non si controllano.
    #[serde(default = "default_true")]
    pub nulls_equal: bool,
}

/// Verifica che la chiave composta dalle colonne configurate sia unica;
/// restituisce il batch invariato.
///
/// Con `nulls_equal=true` i null contano come chiave (un solo null ammesso),
/// con `false` le righe con null nella chiave sono saltate. Tutte le righe dei
/// gruppi duplicati, la prima compresa, sono rifiutate e conteggiate.
///
/// L'uguaglianza e' quella della forma testuale di `key_for_row`: su
/// `Float64` ogni `NaN` e' uguale agli altri e `0.0` e' diverso da `-0.0`.
/// Un `Binary` si confronta sui byte, anche se non e' UTF-8 (dove
/// `key_for_row` fallirebbe).
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`) o cella
///   di una colonna fuori dai tipi nativi che non si converte in testo
///   (come `scalar_as_string`: `Date32` o `Timestamp` fuori calendario,
///   chiave di dizionario fuori intervallo);
/// - `DataMapping` con diagnostica per riga: chiave duplicata, causa
///   `validation.duplicate_key`, senza colonna.
// La raccolta completa delle righe rifiutate rende la funzione una sequenza
// lineare sopra il limite di linee: nessuna complessita' logica aggiunta.
#[allow(clippy::too_many_lines)]
pub fn assert_unique(batch: &RecordBatch, config: &AssertUnique) -> Result<RecordBatch> {
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    // Una sola passata con indici di chiave (`visit_key_ids_where`: stessa
    // identita' di `key_for_row` salvo i `Binary` non UTF-8, valore nativo
    // su colonna singola, chiave binaria altrimenti). Con `nulls_equal=false` le righe con un null
    // logico nella chiave sono saltate PRIMA della codifica, come nel
    // percorso testuale: non producono ne' chiavi ne' errori di conversione.
    //
    // `ultima[indice]` e' l'ultima riga vista con quella chiave: e' il valore
    // che la mappa chiave -> riga del percorso testuale restituiva a ogni
    // `insert`, quindi le diagnostiche escono nello stesso ordine.
    //
    // Non c'e' una seconda passata di verifica: la chiave duplicata produce
    // qui le diagnostiche e l'errore di `reject_rows`, e senza duplicati non
    // c'e' altro da verificare.
    let mut ultima: Vec<usize> = Vec::new();
    let mut rejected_rows = HashSet::new();
    let mut rejections = Vec::new();
    visit_key_ids_where(
        batch,
        &indices,
        |row| {
            config.nulls_equal
                || !indices
                    .iter()
                    .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
        },
        |row, indice, nuova| {
            if nuova {
                ultima.push(row);
                return Ok(());
            }
            let slot = ultima.get_mut(indice).ok_or_else(|| {
                PlenoraError::Internal("assert_unique: chiave senza ultima riga".into())
            })?;
            let first_row = std::mem::replace(slot, row);
            if rejected_rows.insert(first_row) {
                rejections.push(RowRejection {
                    row: first_row,
                    cause: "validation.duplicate_key",
                    column: None,
                });
            }
            rejected_rows.insert(row);
            rejections.push(RowRejection {
                row,
                cause: "validation.duplicate_key",
                column: None,
            });
            Ok(())
        },
    )?;
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

/// Config di `table.assert_range`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertRange {
    /// Colonna da controllare (obbligatorio): `Int64`, `UInt64`, `Float64`,
    /// `Decimal128`, `Date32` (giorni dall'epoca), `Date64` (millisecondi
    /// dall'epoca), `Timestamp` di ogni unita' (il valore nativo nell'unita'
    /// della colonna) o `Utf8` letto come numero.
    pub column: String,
    /// Estremo inferiore, letto esatto ([`NumeroConfig`]): un intero JSON
    /// resta intero anche oltre 2^53, un decimale posizionale resta
    /// decimale. L'analisi del contratto pretende almeno uno fra `min` e
    /// `max`, finiti, con `min <= max` sul valore esatto.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub min: Option<NumeroConfig>,
    /// Estremo superiore; come `min`.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub max: Option<NumeroConfig>,
    /// Estremo `min` incluso (assente: incluso). Senza `min` non ha effetto,
    /// e l'analisi dei contratti lo rifiuta.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub inclusive_min: Option<bool>,
    /// Come `inclusive_min`, per `max`.
    #[serde(default, deserialize_with = "crate::mai_null")]
    pub inclusive_max: Option<bool>,
    /// Con `true` le celle nulle passano; con `false` (default) sono
    /// rifiutate.
    #[serde(default)]
    pub allow_null: bool,
}

impl AssertRange {
    /// Regole sulla sola config, condivise da kernel e analisi dei
    /// contratti: almeno uno fra `min` e `max` (senza, l'asserzione non
    /// vincola nulla), estremi finiti con `min <= max`, e `inclusive_min` /
    /// `inclusive_max` solo con l'estremo corrispondente (senza, non
    /// avrebbero effetto).
    ///
    /// # Errors
    ///
    /// `InvalidPlan` per ciascuna delle regole.
    pub fn verifica_parametri(&self) -> Result<()> {
        if self.min.is_none() && self.max.is_none() {
            return Err(PlenoraError::InvalidPlan(
                "assert_range richiede min o max".into(),
            ));
        }
        // `min <= max` sul valore esatto, come confronta il kernel: sui double
        // due interi distinti oltre 2^53 sarebbero uguali.
        if self.min.is_some_and(|value| !value.double().is_finite())
            || self.max.is_some_and(|value| !value.double().is_finite())
            || self.min.zip(self.max).is_some_and(|(min, max)| {
                crate::compare_bounds(min.esatto(), max.esatto()) == Some(Ordering::Greater)
            })
        {
            return Err(PlenoraError::InvalidPlan(
                "estremi di assert_range non validi".into(),
            ));
        }
        if (self.inclusive_min.is_some() && self.min.is_none())
            || (self.inclusive_max.is_some() && self.max.is_none())
        {
            return Err(PlenoraError::InvalidPlan(
                "inclusive_min/inclusive_max senza l'estremo corrispondente".into(),
            ));
        }
        Ok(())
    }
}

/// true se il valore viola i limiti configurati; `compare` confronta il
/// valore con un estremo (`None` = confronto con NaN: nessuna violazione,
/// come i confronti IEEE storici).
fn range_outside(
    config: &AssertRange,
    compare: &mut dyn FnMut(NumericBound) -> Option<Ordering>,
) -> bool {
    let below = config.min.is_some_and(|min| {
        if config.inclusive_min.unwrap_or(true) {
            compare(min.esatto()) == Some(Ordering::Less)
        } else {
            matches!(
                compare(min.esatto()),
                Some(Ordering::Less | Ordering::Equal)
            )
        }
    });
    let above = config.max.is_some_and(|max| {
        if config.inclusive_max.unwrap_or(true) {
            compare(max.esatto()) == Some(Ordering::Greater)
        } else {
            matches!(
                compare(max.esatto()),
                Some(Ordering::Greater | Ordering::Equal)
            )
        }
    });
    below || above
}

/// `true` se la cella e' un valore non finito (inf/NaN).
///
/// Viola sempre l'intervallo, e va deciso PRIMA del confronto: con NaN
/// `partial_cmp` non e' definito, quindi il valore non risulterebbe ne' sotto
/// il minimo ne' sopra il massimo e passerebbe come "dentro".
fn non_finite_cell(array: &dyn Array, row: usize) -> bool {
    if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
        return !values.value(row).is_finite();
    }
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return matches!(
            NumericBound::parse(values.value(row).trim()),
            Some(NumericBound::F64(value)) if !value.is_finite()
        );
    }
    false
}

/// Verifica che i valori della colonna rientrino nei limiti configurati;
/// restituisce il batch invariato.
///
/// I confronti avvengono nel dominio NATIVO di ogni tipo (`scalar_compare`):
/// esatti oltre 2^53 per gli interi, esatti sui decimal, sull'istante per i
/// timestamp. I valori non finiti (inf/NaN) violano sempre l'intervallo; i
/// null sono ammessi solo con `allow_null=true`.
///
/// # Errors
///
/// - `InvalidPlan`: le regole di [`AssertRange::verifica_parametri`];
/// - `Schema`: colonna assente dallo schema (come `column_index`) o valore
///   non confrontabile numericamente (come `scalar_compare`: tipo fuori
///   elenco, testo `Utf8` che non e' un numero); il passo fallisce subito,
///   senza diagnostica per riga;
/// - `DataMapping` con diagnostica per riga: valore fuori intervallo o non
///   finito (causa `validation.value_out_of_range`), null con
///   `allow_null=false` (causa `validation.required_value_missing`).
pub fn assert_range(batch: &RecordBatch, config: &AssertRange) -> Result<RecordBatch> {
    config.verifica_parametri()?;
    let index = column_index(batch, &config.column)?;
    let array = batch.column(index).as_ref();
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        // `None` = riga null (gestita sotto); `Some(true)` = fuori intervallo.
        // Un vincolo e' una DECISIONE sui dati: il confronto non passa mai da
        // `f64` (che oltre 2^53 collassa interi distinti e sui decimal
        // sbaglia l'ordine), ma dal comparatore tipizzato.
        let outside = if crate::is_logically_null(array, row) {
            None
        } else if non_finite_cell(array, row) {
            Some(true)
        } else {
            let mut esito = Ok(false);
            let fuori = range_outside(
                config,
                &mut |bound| match scalar_compare(array, row, bound) {
                    Ok(ordering) => ordering,
                    Err(error) => {
                        if esito.is_ok() {
                            esito = Err(error);
                        }
                        None
                    }
                },
            );
            esito?;
            Some(fuori)
        };
        let Some(outside) = outside else {
            if config.allow_null {
                continue;
            }
            rejections.push(RowRejection {
                row,
                cause: "validation.required_value_missing",
                column: Some(&config.column),
            });
            continue;
        };
        if outside {
            rejections.push(RowRejection {
                row,
                cause: "validation.value_out_of_range",
                column: Some(&config.column),
            });
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

/// Config di `table.assert_regex`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertRegex {
    /// Colonna `Utf8` da controllare (obbligatorio).
    pub column: String,
    /// Regex del crate `regex` (obbligatoria). L'analisi del contratto
    /// rifiuta il pattern vuoto e quello oltre `max_regex_bytes`.
    pub pattern: String,
    /// Con `true` le celle nulle passano; con `false` (default) sono
    /// rifiutate.
    #[serde(default)]
    pub allow_null: bool,
}

/// Verifica che i valori della colonna Utf8 corrispondano alla regex
/// configurata; restituisce il batch invariato.
///
/// La corrispondenza e' una ricerca (`Regex::is_match`): senza `^` e `$`
/// basta che una parte del valore corrisponda.
///
/// # Errors
///
/// - `Schema`: colonna assente dallo schema (come `column_index`) o non di
///   tipo Utf8;
/// - `InvalidPlan`: pattern non una regex valida;
/// - `DataMapping` con diagnostica per riga: valore che non corrisponde
///   (causa `validation.regex_mismatch`), null con `allow_null=false` (causa
///   `validation.required_value_missing`).
pub fn assert_regex(batch: &RecordBatch, config: &AssertRegex) -> Result<RecordBatch> {
    let index = column_index(batch, &config.column)?;
    if batch.column(index).data_type() != &DataType::Utf8 {
        return Err(PlenoraError::Schema(
            "assert_regex richiede una colonna Utf8".into(),
        ));
    }
    let pattern = Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::InvalidPlan(format!("regex non valida: {error}")))?;
    let mut rejections = Vec::new();
    for row in 0..batch.num_rows() {
        match scalar_as_string(batch.column(index).as_ref(), row)? {
            Some(value) if pattern.is_match(&value) => {}
            None if config.allow_null => {}
            None => {
                rejections.push(RowRejection {
                    row,
                    cause: "validation.required_value_missing",
                    column: Some(&config.column),
                });
            }
            Some(_) => {
                rejections.push(RowRejection {
                    row,
                    cause: "validation.regex_mismatch",
                    column: Some(&config.column),
                });
            }
        }
    }
    reject_rows(
        &rejections,
        "righe non conformi; consultare row_diagnostics",
    )?;
    Ok(batch.clone())
}

/// Config di `table.coalesce`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coalesce {
    /// Colonne da cui prendere il valore, in ordine di precedenza
    /// (obbligatorio, almeno una, con tipi Arrow identici).
    pub columns: Vec<String>,
    /// Colonna del risultato (obbligatorio): sostituita nella sua posizione
    /// se esiste, altrimenti aggiunta in coda; sempre nullabile.
    pub output_column: String,
}

/// Prima colonna non nulla (null logico) fra quelle configurate, riga per
/// riga; il risultato sostituisce (o aggiunge) `output_column`.
///
/// Tutte le colonne devono avere lo stesso tipo Arrow; il percorso veloce
/// tipizzato di `cleansing::coalesce_fast` ha semantica identica al percorso
/// generico (`coalesce_generic`).
///
/// # Errors
///
/// - `InvalidPlan`: nome di output non valido (come `validate_output_name`)
///   o lista di colonne vuota;
/// - `ResourceLimit`: overflow degli indici interni del percorso generico
///   (cresce col numero di righe);
/// - `Schema`: colonna assente dallo schema (come `column_index`), tipi
///   Arrow non identici fra le colonne;
/// - `DataMapping` (`arrow error`): errore Arrow nella concat/take del
///   percorso generico o nella costruzione del batch risultante (come
///   `replace_or_append`).
pub fn coalesce(batch: &RecordBatch, config: &Coalesce) -> Result<RecordBatch> {
    validate_output_name(&config.output_column)?;
    if config.columns.is_empty() {
        return Err(PlenoraError::InvalidPlan(
            "coalesce richiede almeno una colonna".into(),
        ));
    }
    let indices = config
        .columns
        .iter()
        .map(|name| column_index(batch, name))
        .collect::<Result<Vec<_>>>()?;
    let data_type = batch.column(indices[0]).data_type().clone();
    if indices
        .iter()
        .any(|index| batch.column(*index).data_type() != &data_type)
    {
        return Err(PlenoraError::Schema(
            "coalesce richiede colonne con tipi Arrow identici".into(),
        ));
    }
    // Percorso veloce tipizzato: copre Int64,
    // Float64, UInt64, Boolean, Utf8 con semantica identica al generico.
    if let Some(values) = crate::cleansing::coalesce_fast(batch, &indices) {
        return replace_or_append(batch, &config.output_column, data_type, true, values);
    }
    let values = coalesce_generic(batch, &indices)?;
    replace_or_append(batch, &config.output_column, data_type, true, values)
}

/// Percorso generico (concat + take): fallback per i tipi non
/// coperti da `cleansing::coalesce_fast` e oracolo dei test di equivalenza.
pub(crate) fn coalesce_generic(batch: &RecordBatch, indices: &[usize]) -> Result<ArrayRef> {
    let arrays = indices
        .iter()
        .map(|index| batch.column(*index).as_ref())
        .collect::<Vec<_>>();
    let combined = plenora_core::arrow::select::concat::concat(&arrays)?;
    let take_indices = (0..batch.num_rows())
        .map(|row| {
            indices
                .iter()
                .position(|index| !crate::is_logically_null(batch.column(*index).as_ref(), row))
                .map(|position| {
                    position
                        .checked_mul(batch.num_rows())
                        .and_then(|offset| offset.checked_add(row))
                        .ok_or_else(|| {
                            PlenoraError::ResourceLimit("overflow indice coalesce".into())
                        })
                })
                .transpose()?
                .map(u64::try_from)
                .transpose()
                .map_err(|_| PlenoraError::ResourceLimit("indice coalesce oltre u64".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(plenora_core::arrow::select::take::take(
        combined.as_ref(),
        &UInt64Array::from(take_indices),
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use plenora_core::arrow::array::{Float64Array, Int64Array, StringArray};
    use plenora_core::arrow::schema::{Field, Schema};

    use super::*;
    use crate::test_support::single_column_batch;

    fn fixture() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("text", DataType::Utf8, true),
                Field::new("id", DataType::Int64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("ok"), None])),
                Arc::new(Int64Array::from(vec![1, 2])),
            ],
        )
        .expect("quality fixture")
    }

    #[test]
    fn assert_range_integer_bounds_are_exact_beyond_2_pow_53() {
        // Classe "confronti via f64": 2^53+1 collassa sul double 2^53; un
        // estremo max = 2^53 (esatto in f64) deve comunque escluderlo.
        let ints = single_column_batch(
            "i",
            Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_992), // 2^53
                Some(9_007_199_254_740_993), // 2^53 + 1
                None,
            ])),
            DataType::Int64,
            true,
        );
        let config = AssertRange {
            column: "i".into(),
            min: Some(9_007_199_254_740_992_i64.into()),
            max: Some(9_007_199_254_740_992_i64.into()),
            inclusive_min: None,
            inclusive_max: None,
            allow_null: true,
        };
        // La riga con 2^53+1 viola il massimo (il null e' ammesso).
        assert!(assert_range(&ints, &config).is_err());
        // Senza la riga oltre 2^53 il vincolo passa.
        let ok = single_column_batch(
            "i",
            Arc::new(Int64Array::from(vec![Some(9_007_199_254_740_992), None])),
            DataType::Int64,
            true,
        );
        assert!(assert_range(&ok, &config).is_ok());

        let uints = single_column_batch(
            "u",
            Arc::new(UInt64Array::from(vec![
                Some(9_007_199_254_740_993_u64), // 2^53 + 1
                Some(9),
            ])),
            DataType::UInt64,
            true,
        );
        // u64 oltre 2^53 contro max = 2^53: violazione esatta (9 < 10 anche
        // in ordinato, mai confronto testuale).
        assert!(assert_range(&uints, &config_u64()).is_err());
        let uints_ok = single_column_batch(
            "u",
            Arc::new(UInt64Array::from(vec![Some(9), Some(10)])),
            DataType::UInt64,
            true,
        );
        assert!(assert_range(&uints_ok, &config_u64_min()).is_ok());
    }

    /// Regressione: gli estremi si leggono esatti dal JSON. Come `f64`,
    /// `max = 2^53 + 1` arrivava come 2^53 e rifiutava il valore 2^53 + 1,
    /// che e' dentro; `min = 2^53 + 1` lasciava passare 2^53, che e' fuori.
    #[test]
    fn assert_range_legge_gli_estremi_interi_esatti_dal_json() {
        let config = |testo: &str| -> AssertRange {
            serde_json::from_str(testo).expect("config assert_range")
        };
        let ints = single_column_batch(
            "i",
            Arc::new(Int64Array::from(vec![Some(9_007_199_254_740_993)])),
            DataType::Int64,
            true,
        );
        let dentro = [
            r#"{"column": "i", "max": 9007199254740993}"#,
            r#"{"column": "i", "min": 9007199254740993}"#,
        ];
        for testo in dentro {
            assert!(assert_range(&ints, &config(testo)).is_ok(), "{testo}");
        }
        let fuori = r#"{"column": "i", "min": 9007199254740994}"#;
        assert!(assert_range(&ints, &config(fuori)).is_err());
        let limite = single_column_batch(
            "i",
            Arc::new(Int64Array::from(vec![Some(9_007_199_254_740_992)])),
            DataType::Int64,
            true,
        );
        let sopra = r#"{"column": "i", "min": 9007199254740993}"#;
        assert!(assert_range(&limite, &config(sopra)).is_err());
        // u64 oltre i64::MAX: l'estremo resta u64 esatto.
        let uints = single_column_batch(
            "u",
            Arc::new(UInt64Array::from(vec![Some(u64::MAX)])),
            DataType::UInt64,
            true,
        );
        let sotto_max = r#"{"column": "u", "max": 18446744073709551614}"#;
        let al_max = r#"{"column": "u", "max": 18446744073709551615}"#;
        assert!(assert_range(&uints, &config(sotto_max)).is_err());
        assert!(assert_range(&uints, &config(al_max)).is_ok());
        // Un decimale posizionale resta decimale: 0.1 non e' il double 0.1.
        let decimali = single_column_batch(
            "d",
            Arc::new(
                plenora_core::arrow::array::Decimal128Array::from(vec![Some(
                    1_000_000_000_000_000_001_i128,
                )])
                .with_precision_and_scale(38, 19)
                .expect("decimal"),
            ),
            DataType::Decimal128(38, 19),
            true,
        );
        let un_decimo = r#"{"column": "d", "max": 0.1}"#;
        assert!(assert_range(&decimali, &config(un_decimo)).is_err());
        // Esponente (revisione Codex): `0.0000001` riscritto da serde_json come
        // `1e-7` resta un decimale esatto, e il Decimal128 1e-7 e' dentro.
        let piccolo = single_column_batch(
            "d",
            Arc::new(
                plenora_core::arrow::array::Decimal128Array::from(vec![Some(1_i128)])
                    .with_precision_and_scale(10, 7)
                    .expect("decimal"),
            ),
            DataType::Decimal128(10, 7),
            true,
        );
        for testo in [
            r#"{"column": "d", "max": 0.0000001}"#,
            r#"{"column": "d", "max": 1e-7, "min": 1e-7}"#,
        ] {
            assert!(assert_range(&piccolo, &config(testo)).is_ok(), "{testo}");
        }
        // Intero oltre u64: arriva come double 1e20, riletto esatto.
        let interi = single_column_batch(
            "i",
            Arc::new(Int64Array::from(vec![Some(i64::MAX)])),
            DataType::Int64,
            true,
        );
        // Oltre la forma esatta il numero si rifiuta alla lettura della config.
        assert!(serde_json::from_str::<AssertRange>(r#"{"column": "i", "max": 1e-128}"#).is_err());
        let oltre = r#"{"column": "i", "min": 100000000000000000000}"#;
        assert!(assert_range(&interi, &config(oltre)).is_err());
        let sotto = r#"{"column": "i", "max": 100000000000000000000, "min": 1.5e3}"#;
        assert!(assert_range(&interi, &config(sotto)).is_ok());
    }

    fn config_u64() -> AssertRange {
        AssertRange {
            column: "u".into(),
            min: None,
            max: Some(9_007_199_254_740_992_i64.into()),
            inclusive_min: None,
            inclusive_max: None,
            allow_null: false,
        }
    }

    fn config_u64_min() -> AssertRange {
        AssertRange {
            column: "u".into(),
            min: Some(9_i64.into()),
            max: None,
            inclusive_min: None,
            inclusive_max: None,
            allow_null: false,
        }
    }

    #[test]
    fn defensive_runtime_guards_remain_fail_closed_without_plan_validation() {
        let input = fixture();
        assert!(assert_regex(
            &input,
            &AssertRegex {
                column: "text".into(),
                pattern: "(".into(),
                allow_null: true,
            },
        )
        .is_err());
        assert!(assert_schema(
            &input,
            &AssertSchema {
                fields: vec![SchemaExpectation {
                    name: "text".into(),
                    data_type: "decimal128".into(),
                    nullable: None,
                }],
                allow_extra: true,
                ordered: true,
            },
        )
        .is_err());
        assert!(assert_not_null(
            &input,
            &AssertNotNull {
                columns: vec!["missing".into()],
            },
        )
        .is_err());
        assert!(coalesce(
            &input,
            &Coalesce {
                columns: Vec::new(),
                output_column: "result".into(),
            },
        )
        .is_err());
    }

    #[test]
    fn assert_regex_distinguishes_null_from_text_mismatch() {
        let batch = single_column_batch(
            "text",
            Arc::new(StringArray::from(vec![None, Some("no")])),
            DataType::Utf8,
            true,
        );
        let error = assert_regex(
            &batch,
            &AssertRegex {
                column: "text".into(),
                pattern: "^yes$".into(),
                allow_null: false,
            },
        )
        .expect_err("righe non conformi accettate");
        let report = error.row_diagnostics().expect("diagnostica regex mancante");
        assert_eq!(report.counts["validation.required_value_missing"], 1);
        assert_eq!(report.counts["validation.regex_mismatch"], 1);
        assert_eq!(report.examples[0].source_index, 0);
        assert_eq!(
            report.examples[0].cause,
            "validation.required_value_missing"
        );
    }

    // -----------------------------------------------------------------------
    // Test-oracolo del percorso a indici di chiave di `assert_unique`: qui
    // sotto un'implementazione di riferimento indipendente, che si ferma al
    // primo duplicato. Ogni scenario confronta l'esito (ok/errore), e il
    // messaggio solo se l'errore non porta diagnostica per riga (colonna
    // mancante, conversione): il riferimento la diagnostica non la produce.
    // -----------------------------------------------------------------------

    /// Oracolo indipendente di `assert_unique`: stesso contratto, percorso
    /// diverso.
    fn assert_unique_reference(batch: &RecordBatch, config: &AssertUnique) -> Result<RecordBatch> {
        let indices = config
            .columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let mut seen = HashSet::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            if !config.nulls_equal
                && indices
                    .iter()
                    .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
            {
                continue;
            }
            if !seen.insert(key_for_row(batch, &indices, row)?) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "assert_unique: duplicato alla riga {row}"
                )));
            }
        }
        Ok(batch.clone())
    }

    fn unique_config(columns: &[&str], nulls_equal: bool) -> AssertUnique {
        AssertUnique {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            nulls_equal,
        }
    }

    fn describe(outcome: &Result<RecordBatch>) -> String {
        match outcome {
            Ok(batch) => format!("ok ({} righe)", batch.num_rows()),
            Err(error) => error.to_string(),
        }
    }

    /// Esito uguale all'oracolo; messaggio identico se l'errore non ha
    /// diagnostica per riga.
    fn assert_unique_equivalent(batch: &RecordBatch, config: &AssertUnique) {
        let reference = assert_unique_reference(batch, config);
        let fast = assert_unique(batch, config);
        match (&reference, &fast) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(expected.num_rows(), actual.num_rows());
                assert_eq!(expected.num_columns(), actual.num_columns());
            }
            (Err(expected), Err(actual)) => {
                if actual.row_diagnostics().is_none() {
                    assert_eq!(expected.to_string(), actual.to_string());
                }
            }
            _ => panic!(
                "esiti divergenti: reference={} fast={}",
                describe(&reference),
                describe(&fast)
            ),
        }
    }

    fn int_batch(ids: Vec<Option<i64>>) -> RecordBatch {
        single_column_batch("id", Arc::new(Int64Array::from(ids)), DataType::Int64, true)
    }

    #[test]
    fn assert_unique_matches_reference_on_duplicate_positions() {
        // Nessun duplicato: output invariato.
        let unique = int_batch((0..1_000).map(Some).collect());
        assert_unique_equivalent(&unique, &unique_config(&["id"], true));
        // Duplicato adiacente in testa (righe 0 e 1): rifiutate entrambe.
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[1] = ids[0];
        let head = int_batch(ids);
        assert_unique_equivalent(&head, &unique_config(&["id"], true));
        let error = assert_unique(&head, &unique_config(&["id"], true)).expect_err("duplicato");
        let report = error.row_diagnostics().expect("diagnostica duplicato");
        assert_eq!(report.observed_total, 2);
        assert_eq!(report.examples[0].source_index, 0);
        assert_eq!(report.examples[1].source_index, 1);
        assert_eq!(report.examples[0].cause, "validation.duplicate_key");
        // Duplicato all'ultima riga (caso peggiore: scansione completa).
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        let last = ids.len() - 1;
        ids[last] = ids[0];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
        // Duplicati distanti (righe 7 e 842).
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[842] = ids[7];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
        // Duplicati adiacenti a meta' batch.
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[501] = ids[500];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
        // Tre righe con la stessa chiave (3, 10, 900).
        let mut ids: Vec<Option<i64>> = (0..1_000).map(Some).collect();
        ids[10] = ids[3];
        ids[900] = ids[3];
        assert_unique_equivalent(&int_batch(ids), &unique_config(&["id"], true));
    }

    #[test]
    fn assert_unique_matches_reference_on_nulls_in_keys() {
        // nulls_equal=true: null == null, due null sono duplicato (riga 3).
        let with_nulls = int_batch(vec![Some(1), None, Some(2), None]);
        assert_unique_equivalent(&with_nulls, &unique_config(&["id"], true));
        // nulls_equal=false: le righe con null nella chiave sono saltate.
        assert_unique_equivalent(&with_nulls, &unique_config(&["id"], false));
        // Solo null: saltati tutti con nulls_equal=false, duplicato alla riga 1
        // con nulls_equal=true.
        let all_null = int_batch(vec![None, None, None]);
        assert_unique_equivalent(&all_null, &unique_config(&["id"], true));
        assert_unique_equivalent(&all_null, &unique_config(&["id"], false));
        // Duplicato non-null oltre i null saltati (nulls_equal=false).
        let batch = int_batch(vec![None, Some(5), None, Some(5)]);
        assert_unique_equivalent(&batch, &unique_config(&["id"], false));
        // Chiave nativa Utf8: i null sono contati come chiave
        // (duplicato alla riga 3) con nulls_equal=true, saltati con false.
        let utf8_nulls = single_column_batch(
            "s",
            Arc::new(StringArray::from(vec![Some("a"), None, Some("b"), None])),
            DataType::Utf8,
            true,
        );
        assert_unique_equivalent(&utf8_nulls, &unique_config(&["s"], true));
        assert_unique_equivalent(&utf8_nulls, &unique_config(&["s"], false));
    }

    #[test]
    fn assert_unique_matches_reference_on_multicolumn_mixed_types() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, true),
                Field::new("tag", DataType::Utf8, true),
                Field::new("val", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(1), Some(1), Some(2)])),
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("a"),
                    Some("b"),
                    Some("a"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    Some(2.5),
                    Some(1.5),
                    Some(1.5),
                ])),
            ],
        )
        .expect("batch misto");
        // (id, tag): duplicato alle righe 0 e 1.
        assert_unique_equivalent(&batch, &unique_config(&["id", "tag"], true));
        // (id, tag, val): la riga 1 differisce su val, nessun duplicato.
        assert_unique_equivalent(&batch, &unique_config(&["id", "tag", "val"], true));
        // Stessa composizione del generico anche con colonna ripetuta.
        assert_unique_equivalent(&batch, &unique_config(&["id", "id"], true));
        // Null in una colonna della chiave composta: null == null.
        let with_null = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, true),
                Field::new("tag", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(1), Some(1)])),
                Arc::new(StringArray::from(vec![None, Some("x"), None])),
            ],
        )
        .expect("batch misto con null");
        assert_unique_equivalent(&with_null, &unique_config(&["id", "tag"], true));
        assert_unique_equivalent(&with_null, &unique_config(&["id", "tag"], false));
    }

    #[test]
    fn assert_unique_matches_reference_on_nan_and_negative_zero() {
        // NaN serializza come "NaN": due NaN sono duplicati (riga 3).
        let nans = single_column_batch(
            "v",
            Arc::new(Float64Array::from(vec![
                Some(1.0),
                Some(f64::NAN),
                Some(2.0),
                Some(f64::NAN),
            ])),
            DataType::Float64,
            true,
        );
        assert_unique_equivalent(&nans, &unique_config(&["v"], true));
        // 0.0 -> "0" e -0.0 -> "-0": chiavi diverse, nessun duplicato.
        let zeros = single_column_batch(
            "v",
            Arc::new(Float64Array::from(vec![Some(0.0), Some(-0.0)])),
            DataType::Float64,
            true,
        );
        assert_unique_equivalent(&zeros, &unique_config(&["v"], true));
    }

    #[test]
    fn assert_unique_matches_reference_on_edge_configs() {
        // Input vuoto: ok per entrambi.
        let empty = int_batch(Vec::new());
        assert_unique_equivalent(&empty, &unique_config(&["id"], true));
        // Nessuna colonna in chiave: chiave vuota per ogni riga, duplicato
        // alla riga 1 (stessa semantica del generico).
        let batch = int_batch(vec![Some(1), Some(2), Some(3)]);
        assert_unique_equivalent(&batch, &unique_config(&[], true));
        // Colonna mancante: stesso errore di schema.
        assert_unique_equivalent(&batch, &unique_config(&["missing"], true));
        let error = assert_unique(&batch, &unique_config(&["missing"], true))
            .expect_err("colonna mancante");
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn assert_unique_matches_reference_across_key_types() {
        use plenora_core::arrow::array::{BinaryArray, BooleanArray, Date32Array, UInt64Array};
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("u", DataType::UInt64, true),
                Field::new("b", DataType::Boolean, true),
                Field::new("f", DataType::Float64, true),
                Field::new("s", DataType::Utf8, true),
                Field::new("d", DataType::Date32, true),
                Field::new("bin", DataType::Binary, true),
            ])),
            vec![
                Arc::new(UInt64Array::from(vec![Some(7), Some(8), Some(7)])),
                Arc::new(BooleanArray::from(vec![
                    Some(true),
                    Some(false),
                    Some(true),
                ])),
                Arc::new(Float64Array::from(vec![Some(1.25), Some(2.5), Some(1.25)])),
                Arc::new(StringArray::from(vec![Some("x"), Some("y"), Some("x")])),
                Arc::new(Date32Array::from(vec![
                    Some(19_000),
                    Some(19_001),
                    Some(19_000),
                ])),
                Arc::new(BinaryArray::from(vec![
                    Some(&b"aa"[..]),
                    Some(&b"bb"[..]),
                    Some(&b"aa"[..]),
                ])),
            ],
        )
        .expect("batch multi-tipo");
        // Duplicato alla riga 2 su ogni colonna, singolarmente (chiave
        // nativa, o binaria per Date32 e Binary) e composta.
        for columns in [
            vec!["u"],
            vec!["b"],
            vec!["f"],
            vec!["s"],
            vec!["d"],
            vec!["bin"],
            vec!["u", "b", "f", "s", "d", "bin"],
        ] {
            assert_unique_equivalent(&batch, &unique_config(&columns, true));
        }
    }

    #[test]
    fn assert_unique_matches_reference_at_scale_with_planted_duplicate() {
        // 100k righe (id, grp): duplicato piantato alla riga 75_000.
        let rows = 100_000_usize;
        let mut ids: Vec<Option<i64>> = Vec::with_capacity(rows);
        let mut groups: Vec<Option<&str>> = Vec::with_capacity(rows);
        for row in 0..rows {
            ids.push(Some(i64::try_from(row).expect("indice i64")));
            groups.push(Some(if row % 3 == 0 { "a" } else { "b" }));
        }
        ids[75_000] = ids[25_000];
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, true),
                Field::new("grp", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(groups)),
            ],
        )
        .expect("batch scala");
        assert_unique_equivalent(&batch, &unique_config(&["id", "grp"], true));
        // Senza duplicato (chiave composta unica): ok, output invariato.
        let unique_batch = RecordBatch::try_new(
            batch.schema(),
            vec![
                Arc::new(Int64Array::from(
                    (0..rows)
                        .map(|row| Some(i64::try_from(row).expect("i64")))
                        .collect::<Vec<_>>(),
                )),
                batch.column(1).clone(),
            ],
        )
        .expect("batch scala unico");
        assert_unique_equivalent(&unique_batch, &unique_config(&["id", "grp"], true));
    }

    /// Oracolo del percorso diagnostico di `assert_unique` com'era con le
    /// chiavi testuali: `key_for_row` per riga, mappa chiave -> ultima riga
    /// vista, rifiuti nell'ordine di scansione, poi `reject_rows`. Il percorso
    /// a indici di chiave deve dare lo stesso esito e le stesse diagnostiche.
    fn assert_unique_text_diagnostics(
        batch: &RecordBatch,
        config: &AssertUnique,
    ) -> Result<RecordBatch> {
        let indices = config
            .columns
            .iter()
            .map(|name| column_index(batch, name))
            .collect::<Result<Vec<_>>>()?;
        let mut diagnostic_seen = std::collections::HashMap::new();
        let mut rejected_rows = HashSet::new();
        let mut rejections = Vec::new();
        for row in 0..batch.num_rows() {
            if !config.nulls_equal
                && indices
                    .iter()
                    .any(|index| crate::is_logically_null(batch.column(*index).as_ref(), row))
            {
                continue;
            }
            if let Some(first_row) = diagnostic_seen.insert(key_for_row(batch, &indices, row)?, row)
            {
                if rejected_rows.insert(first_row) {
                    rejections.push(RowRejection {
                        row: first_row,
                        cause: "validation.duplicate_key",
                        column: None,
                    });
                }
                rejected_rows.insert(row);
                rejections.push(RowRejection {
                    row,
                    cause: "validation.duplicate_key",
                    column: None,
                });
            }
        }
        reject_rows(
            &rejections,
            "righe non conformi; consultare row_diagnostics",
        )?;
        Ok(batch.clone())
    }

    #[test]
    fn assert_unique_ha_le_diagnostiche_del_percorso_testuale() {
        use plenora_core::arrow::array::{types::Int32Type, DictionaryArray, Int32Array};

        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        for rows in [0_usize, 1, 2, 9, 60, 300] {
            let ints = (0..rows)
                .map(|_| match next(6) {
                    0 => None,
                    value => Some(i64::try_from(value).expect("intero") - 3),
                })
                .collect::<Vec<_>>();
            let floats = (0..rows)
                .map(|_| match next(7) {
                    0 => None,
                    1 => Some(f64::NAN),
                    2 => Some(f64::from_bits(0x7ff8_0000_0000_0042)),
                    3 => Some(-0.0),
                    4 => Some(0.0),
                    5 => Some(-f64::NAN),
                    _ => Some(2.5),
                })
                .collect::<Vec<_>>();
            let texts = (0..rows)
                .map(|_| {
                    [None, Some("a"), Some("ab"), Some("bc"), Some("c"), Some("")]
                        [usize::try_from(next(6)).expect("indice")]
                })
                .collect::<Vec<_>>();
            let dictionary_keys = (0..rows)
                .map(|_| match next(4) {
                    0 => None,
                    value => Some(i32::try_from(value).expect("chiave") - 1),
                })
                .collect::<Vec<_>>();
            let dictionary = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(dictionary_keys),
                Arc::new(StringArray::from(vec![Some("x"), None, Some("")])),
            )
            .expect("dizionario");
            let batch = RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("i", DataType::Int64, true),
                    Field::new("f", DataType::Float64, true),
                    Field::new("s", DataType::Utf8, true),
                    Field::new(
                        "d",
                        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                        true,
                    ),
                ])),
                vec![
                    Arc::new(Int64Array::from(ints)),
                    Arc::new(Float64Array::from(floats)),
                    Arc::new(StringArray::from(texts)),
                    Arc::new(dictionary),
                ],
            )
            .expect("batch");
            for columns in [
                vec!["i"],
                vec!["f"],
                vec!["s"],
                vec!["d"],
                vec!["i", "s"],
                vec!["s", "i"],
                vec!["f", "d"],
                vec!["i", "f", "s", "d"],
            ] {
                for nulls_equal in [true, false] {
                    let config = unique_config(&columns, nulls_equal);
                    let expected = assert_unique_text_diagnostics(&batch, &config);
                    let actual = assert_unique(&batch, &config);
                    assert_eq!(
                        describe(&expected),
                        describe(&actual),
                        "{columns:?} nulls_equal={nulls_equal}"
                    );
                    assert_eq!(
                        expected
                            .as_ref()
                            .err()
                            .and_then(PlenoraError::row_diagnostics),
                        actual
                            .as_ref()
                            .err()
                            .and_then(PlenoraError::row_diagnostics),
                        "{columns:?} nulls_equal={nulls_equal}"
                    );
                }
            }
        }
    }
}
