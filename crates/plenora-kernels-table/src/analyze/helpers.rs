//! Helper condivisi degli analyzer per-op: deserializzazione tipizzata
//! fail-closed, propagazione della geometria (D16), merge dei metadata
//! (R2.4) e costruzione del `DataContract` di output.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, FieldAllocator, FieldId,
    GeometryColumnContract, PropertyConfidence, PropertyScope,
};
use plenora_core::{PlenoraError, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::validate_output_name;

// ---------------------------------------------------------------------------
// Helper condivisi
// ---------------------------------------------------------------------------

/// Deserializza la config tipizzata (fail-closed: `deny_unknown_fields`).
pub(in crate::analyze) fn typed<T: DeserializeOwned>(op: &str, config: &Value) -> Result<T> {
    serde_json::from_value(config.clone())
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: config non valida: {error}")))
}

pub(in crate::analyze) fn contract_error<T>(op: &str, message: impl Into<String>) -> Result<T> {
    Err(PlenoraError::InvalidPlan(format!(
        "{op}: {}",
        message.into()
    )))
}

pub(in crate::analyze) fn unsupported<T>(op: &str, message: impl Into<String>) -> Result<T> {
    Err(PlenoraError::Unsupported(format!(
        "{op}: {}",
        message.into()
    )))
}

/// Campo per nome, con errore puntuale se assente (replica `column_index`).
pub(in crate::analyze) fn field_of<'a>(
    op: &str,
    input: &'a DataContract,
    name: &str,
) -> Result<&'a Field> {
    input
        .schema
        .field_with_name(name)
        .map_err(|_| PlenoraError::InvalidPlan(format!("{op}: colonna non trovata: {name}")))
}

/// Verifica che un campo sia leggibile come scalare testuale, timezone
/// COMPRESA.
///
/// Delega a [`crate::validate_text_convertible`] — la stessa funzione che
/// usano i kernel — e riporta l'errore nella categoria che questi
/// analizzatori usano per un contratto non soddisfacibile (`InvalidPlan`).
pub(in crate::analyze) fn require_scalar_string_field(op: &str, field: &Field) -> Result<()> {
    crate::validate_text_convertible(field.data_type(), field.name()).map_err(|errore| {
        PlenoraError::InvalidPlan(format!(
            "{op}: non leggibile come scalare testuale: {errore}"
        ))
    })
}

/// Tipi leggibili da `scalar_as_f64` (profilo numerico).
const fn is_numeric(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Float64
            | DataType::Int64
            | DataType::UInt64
            | DataType::Date32
            | DataType::Timestamp(TimeUnit::Millisecond, _)
            | DataType::Decimal128(_, _)
            | DataType::Utf8
    )
}

pub(in crate::analyze) fn require_scalar_string(
    op: &str,
    input: &DataContract,
    name: &str,
) -> Result<()> {
    require_scalar_string_field(op, field_of(op, input, name)?)
}

pub(in crate::analyze) fn require_numeric(
    op: &str,
    input: &DataContract,
    name: &str,
) -> Result<()> {
    let field = field_of(op, input, name)?;
    if is_numeric(field.data_type()) {
        Ok(())
    } else {
        contract_error(
            op,
            format!(
                "colonna {name}: tipo {:?} non convertibile in numero",
                field.data_type()
            ),
        )
    }
}

pub(in crate::analyze) fn require_utf8(op: &str, input: &DataContract, name: &str) -> Result<()> {
    let field = field_of(op, input, name)?;
    if field.data_type() == &DataType::Utf8 {
        Ok(())
    } else {
        contract_error(op, format!("la colonna {name} deve essere Utf8"))
    }
}

pub(in crate::analyze) fn check_output_name(op: &str, name: &str) -> Result<()> {
    validate_output_name(name)
        .map_err(|_| PlenoraError::InvalidPlan(format!("{op}: nome colonna non valido: {name:?}")))
}

/// `round(rows * fraction)` con l'aritmetica f64 del kernel `sample`
/// (i cast riflettono volutamente la sua semantica).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(in crate::analyze) fn round_scaled(rows: u64, fraction: f64) -> u64 {
    ((rows as f64) * fraction).round().min(rows as f64) as u64
}

/// Semantica `replace_or_append`: sostituisce in posizione se il nome esiste,
/// altrimenti appende. Restituisce `true` se ha sovrascritto.
fn upsert(fields: &mut Vec<Field>, name: &str, data_type: DataType, nullable: bool) -> bool {
    if let Some(existing) = fields.iter_mut().find(|field| field.name() == name) {
        *existing = Field::new(name, data_type, nullable);
        true
    } else {
        fields.push(Field::new(name, data_type, nullable));
        false
    }
}

/// Come `upsert`, ma assegna anche un `FieldId` nuovo alla colonna derivata.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` a spazio dei `FieldId` esaurito
/// ([`FieldAllocator::derive`]).
pub(in crate::analyze) fn produce(
    fields: &mut Vec<Field>,
    alloc: &mut FieldAllocator,
    name: &str,
    data_type: DataType,
    nullable: bool,
) -> Result<bool> {
    alloc.derive(name)?;
    Ok(upsert(fields, name, data_type, nullable))
}

pub(in crate::analyze) fn clone_fields(input: &DataContract) -> Vec<Field> {
    input
        .schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect()
}

/// Propaga la colonna geometrica se sopravvive come colonna logica inalterata
/// (stesso tipo dell'input) sotto il nome `preserved_as`; `None` se la colonna
/// e' stata eliminata o sovrascritta (colonna derivata -> nuovo `FieldId`, D16).
pub(in crate::analyze) fn propagate_geometry(
    input: &DataContract,
    output: &Schema,
    preserved_as: Option<&str>,
) -> Option<GeometryColumnContract> {
    let geometry = input.geometries.first()?;
    let name = preserved_as?;
    let source = input.schema.field_with_name(&geometry.name).ok()?;
    let field = output.field_with_name(name).ok()?;
    if field.data_type() != source.data_type() {
        return None;
    }
    Some(GeometryColumnContract {
        field_id: geometry.field_id,
        name: name.to_owned(),
        crs: geometry.crs.clone(),
        dimensions: geometry.dimensions,
        encoding: geometry.encoding,
        nullable: field.is_nullable(),
        // Colonna preservata identica (R2.4 identity-preserving): la
        // dichiarazione dei tipi si propaga invariata come le altre
        // proprieta'.
        types: geometry.types.clone(),
    })
}

/// Unisce le geometrie dei due rami di un'op binaria: la v1 ammette al
/// massimo una colonna geometrica per arco (D16), due sopravvissute ->
/// errore fail-closed.
pub(in crate::analyze) fn merge_geometry(
    op: &str,
    left: Option<GeometryColumnContract>,
    right: Option<GeometryColumnContract>,
) -> Result<Option<GeometryColumnContract>> {
    match (left, right) {
        (Some(_), Some(_)) => contract_error(
            op,
            "due colonne geometriche in output: la v1 ne ammette al massimo una (D16)",
        ),
        (left, right) => Ok(left.or(right)),
    }
}

/// Merge R2.4 dei metadata di schema di due sorgenti.
///
/// Per le op con due schemi di input (join e varianti, `union_distinct`,
/// `table_diff`): chiave presente in una sola sorgente -> copiata; presente
/// in entrambe con lo stesso valore -> copiata; presente in entrambe con
/// valori diversi -> errore `InvalidPlan` (conflitto fra sorgenti = errore, mai
/// precedenza implicita; il messaggio nomina la chiave, mai i valori).
/// Merge dei metadata di schema di N sorgenti (R2.4): una chiave presente
/// in una sola sorgente e' copiata; presente in piu' sorgenti con lo
/// stesso valore e' copiata; con valori diversi e' un errore che nomina
/// SOLO la chiave (mai i valori, regola 8). Le sorgenti sono esaminate in
/// ordine di dichiarazione e le chiavi di ciascuna in ordine
/// lessicografico: il primo conflitto riportato e' deterministico
/// (ADR-0001), mai dipendente dall'ordine di iterazione delle `HashMap`.
fn merge_metadata_maps(
    op: &str,
    merged: &mut HashMap<String, String>,
    right: &HashMap<String, String>,
) -> Result<()> {
    let mut right_keys: Vec<_> = right.keys().collect();
    right_keys.sort();
    for key in right_keys {
        let value = &right[key];
        match merged.get(key) {
            None => {
                merged.insert(key.clone(), value.clone());
            }
            Some(existing) if existing == value => {}
            Some(_) => {
                return contract_error(
                    op,
                    format!("metadata di schema in conflitto sulla chiave {key:?}"),
                );
            }
        }
    }
    Ok(())
}

pub(in crate::analyze) fn merge_schema_metadata(
    op: &str,
    left: &Schema,
    right: &Schema,
) -> Result<HashMap<String, String>> {
    let mut merged = left.metadata().clone();
    merge_metadata_maps(op, &mut merged, right.metadata())?;
    Ok(merged)
}

/// Merge N-ario per `concat`: come [`merge_schema_metadata`] sulle
/// sorgenti in ordine di dichiarazione.
pub(in crate::analyze) fn merge_schema_metadata_many(
    op: &str,
    inputs: &[DataContract],
) -> Result<HashMap<String, String>> {
    let mut merged = HashMap::new();
    for input in inputs {
        merge_metadata_maps(op, &mut merged, input.schema.metadata())?;
    }
    Ok(merged)
}

pub(in crate::analyze) fn finish(
    schema: Schema,
    geometry: Option<GeometryColumnContract>,
    active: Option<FieldId>,
    properties: ContractProperties,
) -> Result<DataContract> {
    DataContract::new(
        Arc::new(schema),
        geometry.into_iter().collect(),
        active,
        properties,
    )
}

/// Proprieta' delle op 1:1 che si limitano ad appendere colonne: `row_count`
/// invariato, `sorted_by` preservato solo se nessuna colonna esistente e'
/// stata sovrascritta (la sovrascrittura puo' toccare una chiave).
fn append_props(input: &DataContract, overwritten: bool) -> ContractProperties {
    ContractProperties {
        sorted_by: if overwritten {
            None
        } else {
            input.properties.sorted_by.clone()
        },
        row_count: input.properties.row_count.clone(),
    }
}

/// Solo `row_count` (op che modificano valori ma non righe/ordine).
pub(in crate::analyze) fn rows_only(input: &DataContract) -> ContractProperties {
    ContractProperties {
        sorted_by: None,
        row_count: input.properties.row_count.clone(),
    }
}

/// Solo `sorted_by` (op che rimuovono righe preservando l'ordine relativo).
pub(in crate::analyze) fn sorted_only(input: &DataContract) -> ContractProperties {
    ContractProperties {
        sorted_by: input.properties.sorted_by.clone(),
        row_count: None,
    }
}

/// `sorted_by = Proven(chiavi, Stream)`: op blocking che riordina l'intero
/// stream di output (Architetture.md par. 4.3; `execution_class` Blocking).
pub(in crate::analyze) const fn proven_sorted(
    keys: Vec<FieldId>,
) -> ContractProperty<Vec<FieldId>> {
    ContractProperty::new(PropertyConfidence::Proven(keys), PropertyScope::Stream)
}

/// Trasforma un `row_count` noto mantenendo provenienza e scope.
pub(in crate::analyze) fn map_row_count(
    input: &DataContract,
    transform: impl Fn(u64) -> u64,
) -> Option<ContractProperty<u64>> {
    let property = input.properties.row_count.as_ref()?;
    let confidence = match &property.confidence {
        PropertyConfidence::Proven(value) => PropertyConfidence::Proven(transform(*value)),
        PropertyConfidence::Declared(value) => PropertyConfidence::Declared(transform(*value)),
        PropertyConfidence::Estimated(value) => PropertyConfidence::Estimated(transform(*value)),
        PropertyConfidence::Unknown => PropertyConfidence::Unknown,
    };
    Some(ContractProperty::new(confidence, property.scope))
}

/// Elimina `sorted_by` se una delle sue chiavi e' la geometria eliminata.
pub(in crate::analyze) fn scrub_dropped_geometry(
    mut properties: ContractProperties,
    dropped: Option<FieldId>,
) -> ContractProperties {
    if let Some(id) = dropped {
        if properties
            .sorted_by
            .as_ref()
            .and_then(|property| property.confidence.value())
            .is_some_and(|keys| keys.contains(&id))
        {
            properties.sorted_by = None;
        }
    }
    properties
}

/// Analisi comune delle op 1:1 che appendono/sostituiscono colonne calcolate:
/// schema = input + colonne prodotte (semantica `replace_or_append`),
/// metadata preservati, geometria propagata se non sovrascritta.
pub(in crate::analyze) fn analyze_append(
    input: &DataContract,
    alloc: &mut FieldAllocator,
    produced: &[(String, DataType, bool)],
) -> Result<DataContract> {
    let mut fields = clone_fields(input);
    let mut overwritten = false;
    let mut geometry_overwritten = false;
    for (name, data_type, nullable) in produced {
        let replaced = produce(&mut fields, alloc, name, data_type.clone(), *nullable)?;
        overwritten |= replaced;
        if replaced && input.geometries.first().is_some_and(|g| &g.name == name) {
            geometry_overwritten = true;
        }
    }
    let schema = Schema::new_with_metadata(fields, input.schema.metadata().clone());
    let preserved = if geometry_overwritten {
        None
    } else {
        input.geometries.first().map(|g| g.name.as_str())
    };
    let geometry = propagate_geometry(input, &schema, preserved);
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(schema, geometry, active, append_props(input, overwritten))
}
