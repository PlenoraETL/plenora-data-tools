//! Helper condivisi degli analizzatori per operazione: deserializzazione
//! tipizzata fail-closed, controlli della config, propagazione della colonna
//! geometrica, unione dei metadati di schema e costruzione del
//! `DataContract` di output.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, FieldAllocator, FieldId,
    GeometryColumnContract, NullPlacement, PropertyConfidence, PropertyScope, SortDirection,
    SortOrder,
};
use plenora_core::{PlenoraError, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::float64_source::dominio_numerico;
use crate::validate_output_name;

// ---------------------------------------------------------------------------
// Helper condivisi
// ---------------------------------------------------------------------------

/// Deserializza la config tipizzata (fail-closed: `deny_unknown_fields`).
pub(in crate::analyze) fn typed<T: DeserializeOwned>(op: &str, config: &Value) -> Result<T> {
    serde_json::from_value(config.clone()).map_err(|error| config_non_valida(op, &error))
}

/// L'errore di piano di una config che serde rifiuta, senza il testo di
/// serde, che cita i valori scritti
/// ([`plenora_core::json::descrivi_errore_config`]).
pub(in crate::analyze) fn config_non_valida(op: &str, error: &serde_json::Error) -> PlenoraError {
    PlenoraError::InvalidPlan(format!(
        "{op}: config non valida: {}",
        plenora_core::json::descrivi_errore_config(error)
    ))
}

pub(in crate::analyze) fn contract_error<T>(op: &str, message: impl Into<String>) -> Result<T> {
    Err(PlenoraError::InvalidPlan(format!(
        "{op}: {}",
        message.into()
    )))
}

/// Un controllo condiviso con il kernel, con l'operazione davanti al
/// messaggio d'errore di piano.
pub(in crate::analyze) fn con_op<T>(op: &str, esito: Result<T>) -> Result<T> {
    esito.map_err(|errore| match errore {
        PlenoraError::InvalidPlan(messaggio) => {
            PlenoraError::InvalidPlan(format!("{op}: {messaggio}"))
        }
        altro => altro,
    })
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
    // Una tabella sola per tutto il crate: l'analizzatore e i kernel devono
    // accettare gli stessi tipi, o un piano passa l'analisi e cade in
    // esecuzione.
    if dominio_numerico(field.data_type()) {
        Ok(())
    } else {
        contract_error(
            op,
            format!(
                "colonna {name}: tipo {} non convertibile in numero",
                plenora_core::tipo_arrow::descrivi_tipo(field.data_type())
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

/// Lista di nomi di colonna della config: non vuota (salvo `allow_empty`),
/// al piu' `max` nomi, senza ripetizioni, ogni nome valido.
///
/// Una ripetizione non e' mai innocua: nei kernel diventa una chiave doppia,
/// una colonna prodotta due volte o una regola in cui «vince l'ultimo».
pub(in crate::analyze) fn check_name_list(
    op: &str,
    names: &[String],
    max: usize,
    label: &str,
    allow_empty: bool,
) -> Result<()> {
    if !allow_empty && names.is_empty() {
        return contract_error(op, format!("{label} vuoto"));
    }
    if names.len() > max {
        return contract_error(op, format!("{label}: numero di colonne oltre il limite"));
    }
    let mut seen: HashSet<&str> = HashSet::with_capacity(names.len());
    for name in names {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("{label}: colonna ripetuta: {name}"));
        }
        check_output_name(op, name)?;
    }
    Ok(())
}

/// Un testo della config entro `max` byte.
pub(in crate::analyze) fn check_text_len(
    op: &str,
    text: &str,
    max: usize,
    label: &str,
) -> Result<()> {
    if text.len() > max {
        return contract_error(op, format!("{label} oltre il limite di byte"));
    }
    Ok(())
}

/// Un valore JSON della config che un kernel scrive in una cella (come
/// testo: una stringa com'e', `null` vuoto, gli altri con il loro testo
/// JSON) entro `max` byte.
pub(in crate::analyze) fn check_json_text(
    op: &str,
    value: &Value,
    max: usize,
    label: &str,
) -> Result<()> {
    let byte = match value {
        Value::String(testo) => testo.len(),
        Value::Null => 0,
        altro => altro.to_string().len(),
    };
    if byte > max {
        return contract_error(op, format!("{label} oltre il limite di byte"));
    }
    Ok(())
}

/// Un conteggio della config entro `max_rows`.
pub(in crate::analyze) fn check_rows(
    op: &str,
    value: u64,
    max_rows: usize,
    label: &str,
) -> Result<()> {
    if u64::try_from(max_rows).is_ok_and(|max| value > max) {
        return contract_error(op, format!("{label} oltre max_rows"));
    }
    Ok(())
}

/// Ogni colonna nominata esiste ed e' leggibile come scalare testuale.
pub(in crate::analyze) fn require_scalar_strings<'a>(
    op: &str,
    input: &DataContract,
    names: impl IntoIterator<Item = &'a String>,
) -> Result<()> {
    names
        .into_iter()
        .try_for_each(|name| require_scalar_string(op, input, name))
}

/// `round(rows * fraction)` con l'aritmetica f64 del kernel `sample`
/// (i cast riflettono volutamente la sua semantica). Il chiamante verifica
/// `rows <= 2^53`: conversione del conteggio e ritorno sono esatti.
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
pub(in crate::analyze) fn produce(
    fields: &mut Vec<Field>,
    name: &str,
    data_type: DataType,
    nullable: bool,
) -> bool {
    if let Some(existing) = fields.iter_mut().find(|field| field.name() == name) {
        *existing = Field::new(name, data_type, nullable);
        true
    } else {
        fields.push(Field::new(name, data_type, nullable));
        false
    }
}

pub(in crate::analyze) fn clone_fields(input: &DataContract) -> Vec<Field> {
    input
        .schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect()
}

/// Propaga ogni geometria attraverso la corrispondenza esplicita dei nomi.
pub(in crate::analyze) fn propagate_geometry(
    input: &DataContract,
    output: &Schema,
    mapping: impl Fn(&GeometryColumnContract) -> Option<String>,
) -> Vec<GeometryColumnContract> {
    input
        .geometries
        .iter()
        .filter_map(|geometry| {
            let name = mapping(geometry)?;
            let source = input.schema.field_with_name(&geometry.name).ok()?;
            let field = output.field_with_name(&name).ok()?;
            if field.data_type() != source.data_type() {
                return None;
            }
            let mut result = geometry.clone();
            result.name = name;
            result.nullable = field.is_nullable();
            Some(result)
        })
        .collect()
}

/// Le primarie verticali devono alimentare la stessa colonna da tutti gli ingressi.
pub(in crate::analyze) fn vertical_primary(
    inputs: &[DataContract],
    geometries: &[GeometryColumnContract],
) -> Option<FieldId> {
    let name = &inputs.first()?.active_geometry_column()?.name;
    if inputs.iter().all(|input| {
        input
            .active_geometry_column()
            .is_some_and(|g| &g.name == name)
    }) {
        geometries
            .iter()
            .find(|g| &g.name == name)
            .map(|g| g.field_id)
    } else {
        None
    }
}

/// Unione dei metadati di schema di N sorgenti: la stessa regola dei kernel
/// (`crate::unisci_metadata_schema`), con l'errore di piano dell'analisi.
///
/// Una chiave su una sola sorgente, o con lo stesso valore su piu' sorgenti,
/// e' copiata; valori diversi sono un errore `InvalidPlan` che nomina solo
/// la chiave (AGENTS.md, regola 5), mai precedenza implicita. Sorgenti in
/// ordine di dichiarazione e chiavi in ordine lessicografico: il primo
/// conflitto e' deterministico.
fn merge_metadata_maps<'a>(
    op: &str,
    sorgenti: impl IntoIterator<Item = &'a plenora_core::arrow::Metadata>,
) -> Result<HashMap<String, String>> {
    crate::unisci_metadata_schema(sorgenti).or_else(|error| match error {
        PlenoraError::Schema(message) => contract_error(op, message),
        other => Err(other.con_contesto(op)),
    })
}

pub(in crate::analyze) fn merge_schema_metadata(
    op: &str,
    left: &Schema,
    right: &Schema,
) -> Result<HashMap<String, String>> {
    merge_metadata_maps(op, [left.metadata(), right.metadata()])
}

/// Merge N-ario per `concat`: come [`merge_schema_metadata`] sulle
/// sorgenti in ordine di dichiarazione.
pub(in crate::analyze) fn merge_schema_metadata_many(
    op: &str,
    inputs: &[DataContract],
) -> Result<HashMap<String, String>> {
    merge_metadata_maps(op, inputs.iter().map(|input| input.schema.metadata()))
}

/// Origini di colonne che l'operazione dichiara conservate, in ordine d'uscita.
pub(in crate::analyze) fn preserved_origins<'a>(
    input: &DataContract,
    sources: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<plenora_core::contract::FieldOrigin>> {
    sources
        .into_iter()
        .map(|source| {
            input
                .field_ids
                .get(source)
                .copied()
                .map(plenora_core::contract::FieldOrigin::Preserved)
                .ok_or_else(|| PlenoraError::Schema("identita' sorgente assente".into()))
        })
        .collect()
}

// Il costruttore consuma lo schema logico e ne ricostruisce i metadati pubblici.
#[allow(clippy::needless_pass_by_value)]
pub(in crate::analyze) fn finish(
    origins: Vec<plenora_core::contract::FieldOrigin>,
    schema: Schema,
    geometry: impl IntoIterator<Item = GeometryColumnContract>,
    active: Option<FieldId>,
    properties: ContractProperties,
    allocator: &mut FieldAllocator,
) -> Result<DataContract> {
    let mut geometry: Vec<_> = geometry.into_iter().collect();
    geometry.sort_by_key(|g| schema.index_of(&g.name).unwrap_or(usize::MAX));
    let primary = active
        .and_then(|id| geometry.iter().find(|g| g.field_id == id))
        .map(|g| schema.index_of(&g.name))
        .transpose()?;
    let originals = schema
        .fields()
        .iter()
        .zip(&origins)
        .map(|(field, origin)| {
            let inherited = match origin {
                plenora_core::contract::FieldOrigin::Preserved(id) => {
                    allocator.public_identity(*id)
                }
                plenora_core::contract::FieldOrigin::New => None,
            };
            inherited.cloned().unwrap_or_else(|| {
                field
                    .metadata()
                    .get(plenora_core::contract::arrow_metadata::PLENORA_FIELD_ID_KEY)
                    .cloned()
            })
        })
        .collect::<Vec<_>>();
    let schema = crate::geometry_provenance::emit(
        schema.fields().iter().map(|f| f.as_ref().clone()).collect(),
        primary,
        geometry.len(),
        schema.metadata().clone().into_iter().collect(),
    )?;
    let output = DataContract::new(
        Arc::new(schema),
        origins,
        geometry,
        active,
        properties,
        allocator,
    )?;
    for (field, original) in output.schema.fields().iter().zip(originals) {
        if field
            .metadata()
            .get(plenora_core::contract::arrow_metadata::PLENORA_FIELD_ID_KEY)
            != original.as_ref()
        {
            allocator.record_public_identity(output.field_ids[field.name()], original);
        }
    }
    Ok(output)
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

/// `sorted_by = Proven(ordine, Stream)`: op blocking (`execution_class`
/// Blocking) che riordina l'intera tabella di output.
///
/// L'ordine è quello del kernel `table.sort` (`aggregation::sort`): il
/// comparatore mette il null dopo ogni valore e il discendente rovescia il
/// confronto intero, quindi null in coda in ascendente e in testa in
/// discendente.
pub(in crate::analyze) const fn proven_sorted(
    keys: Vec<FieldId>,
    ascending: bool,
) -> ContractProperty<SortOrder> {
    let (direction, nulls) = if ascending {
        (SortDirection::Ascending, NullPlacement::Last)
    } else {
        (SortDirection::Descending, NullPlacement::First)
    };
    ContractProperty::new(
        PropertyConfidence::Proven(SortOrder {
            keys,
            direction,
            nulls,
        }),
        PropertyScope::Stream,
    )
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
            .is_some_and(|order| order.keys.contains(&id))
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
    analyze_append_inner(input, alloc, produced, None)
}

/// Coalesce conserva la natura geometrica del campo sostituito durante la costruzione.
pub(in crate::analyze) fn analyze_append_inner(
    input: &DataContract,
    alloc: &mut FieldAllocator,
    produced: &[(String, DataType, bool)],
    replacement_metadata: Option<&plenora_core::arrow::Metadata>,
) -> Result<DataContract> {
    let mut fields = clone_fields(input);
    let mut overwritten = false;

    for (name, data_type, nullable) in produced {
        let replaced = produce(&mut fields, name, data_type.clone(), *nullable);
        if let Some(metadata) = replacement_metadata {
            let field = fields
                .iter_mut()
                .find(|field| field.name() == name)
                .ok_or_else(|| PlenoraError::Schema("campo prodotto assente".into()))?;
            *field = field.clone().with_metadata(metadata.clone());
        }
        overwritten |= replaced;
    }
    let schema = Schema::new_with_metadata(fields, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, |g| {
        (!produced.iter().any(|(name, _, _)| name == &g.name)
            || replacement_metadata.is_some_and(|m| !m.is_empty()))
        .then(|| g.name.clone())
    });
    let active = input
        .active_geometry
        .filter(|id| geometry.iter().any(|g| &g.field_id == id));
    finish(
        schema
            .fields()
            .iter()
            .map(|field| {
                if produced.iter().any(|(name, _, _)| name == field.name()) {
                    plenora_core::contract::FieldOrigin::New
                } else {
                    plenora_core::contract::FieldOrigin::Preserved(input.field_ids[field.name()])
                }
            })
            .collect(),
        schema,
        geometry,
        active,
        append_props(input, overwritten),
        alloc,
    )
}

/// Completa il contratto di una colonna che conserva celle geometriche.
/// Il riferimento e' gia' stato verificato prima delle allocazioni.
pub(in crate::analyze) fn derive_geometry(
    mut output: DataContract,
    input: &DataContract,
    source: &str,
    name: &str,
    replaces_primary: bool,
    metadata: plenora_core::arrow::Metadata,
    alloc: &mut FieldAllocator,
) -> Result<DataContract> {
    if metadata.is_empty() {
        return Ok(output);
    }
    let mut fields = clone_fields(&output);
    let index = output.schema.index_of(name)?;
    fields[index] = fields[index].clone().with_metadata(metadata);
    let keys = plenora_core::contract::arrow_metadata::read_geometry_contract_keys(&fields[index])?;
    let crs = if let Some(geometry) = input.geometries.iter().find(|g| g.name == source) {
        geometry.crs.clone()
    } else {
        plenora_core::contract::arrow_schema::contract_crs_from_keys(
            source,
            None,
            &keys,
            plenora_core::crs::resolve_crs,
        )?
    };
    let mut geometry = plenora_core::contract::arrow_schema::geometry_contract_from_field(
        &fields[index],
        crs,
        &keys,
    );
    geometry.field_id = output.field_ids[name];
    output.geometries.retain(|g| g.name != name);
    if input.geometries.is_empty()
        || (replaces_primary
            && input
                .active_geometry_column()
                .is_some_and(|g| g.name == name))
    {
        output.active_geometry = Some(geometry.field_id);
    }
    output.geometries.push(geometry);
    output
        .geometries
        .sort_by_key(|g| output.schema.index_of(&g.name).unwrap_or(usize::MAX));
    let origins = output.field_origins();
    finish(
        origins,
        Schema::new_with_metadata(fields, output.schema.metadata().clone()),
        output.geometries,
        output.active_geometry,
        output.properties,
        alloc,
    )
}
