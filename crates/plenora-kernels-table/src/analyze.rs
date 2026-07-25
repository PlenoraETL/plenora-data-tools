//! Inferenza a secco del `DataContract` di output per le 71 operazioni
//! `table.*` del catalogo (Fase 2A-2, Architetture.md par. 4.3 e 6.1, ADR 5).
//!
//! [`analyze_table_contract`] deserializza la config tipizzata dell'operazione
//! (fail-closed: config non valida -> errore `Contract` puntuale), replica le
//! validazioni statiche del kernel (esistenza colonne, vincoli di tipo,
//! parametri) e inferisce il contratto di output: schema Arrow, propagazione
//! della colonna geometrica (D16) e proprietà (`sorted_by`, `row_count`) con
//! provenienza e scope (D25).
//!
//! Regole di propagazione (D16): una rinomina preserva il `FieldId`, una
//! colonna derivata ne riceve uno nuovo dal [`FieldAllocator`]. La colonna
//! geometrica sopravvive solo se la colonna e' propagata inalterata (stesso
//! tipo, valori passthrough): una sovrascrittura in place (semantica
//! `replace_or_append`) produce una colonna derivata senza metadati
//! `geoarrow.wkb`, quindi il contratto diventa tabellare.
//!
//! Proprieta': v1 deliberatamente conservativa. `sorted_by` e' `Proven` in
//! output solo per le op blocking che riordinano l'intero stream (`sort`,
//! `dedup_advanced`/`rolling_window`/`window_function` con `order_column`);
//! le op che preservano l'ordine delle righe propagano la proprieta' di
//! input inalterata; tutte le altre la eliminano. `row_count` e' propagato
//! solo quando il numero di righe e' esatto (op 1:1, `concat`, `cross_join`,
//! `melt`, `asof_join`, `reconcile`), mai inventato.
//!
//! Op con schema dipendente dai dati (non inferibile a secco):
//! `table.pivot`, `table.transpose` e `table.flatten_json` senza
//! `output_columns` esplicito falliscono con `Unsupported` esplicito —
//! meglio fallire in validazione che indovinare uno schema sbagliato.

// Firma uniforme degli analyzer per-op: il dispatch passa l'allocatore di
// FieldId a ogni operazione, anche a quelle (assert, gate, join) che non
// derivano colonne e quindi non lo usano.
#![allow(clippy::needless_pass_by_ref_mut)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use plenora_core::arrow::schema::{DataType, Field, Schema, TimeUnit};
use plenora_core::catalog::{find_operation, Arity, Family};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, FieldAllocator, FieldId,
    GeometryColumnContract, PropertyConfidence, PropertyScope,
};
use plenora_core::{PlenoraError, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    aggregation, analysis, cleansing, columns, dates, expressions, filtering, formula, fuzzy,
    governance, joins, quality, reshape, security, setops, strings, utility, validate_output_name,
    Limits,
};

/// Numero massimo di nodi AST accettati nell'audit di `table.expression`
/// (limite statico dell'analisi a secco; il kernel riceve il valore dal
/// chiamante, qui non esiste un `Limits` dedicato).
const MAX_EXPRESSION_NODES: usize = 4_096;

/// Inferisce il `DataContract` di output di un'operazione `table.*` a secco
/// (Fase 1 `validate`, Architetture.md par. 6.1 passo 6).
///
/// `op` accetta id canonici e alias legacy (risolti via catalogo);
/// `inputs` deve rispettare l'arieta' dichiarata dal catalogo (unaria,
/// binaria ordinata, N-aria per `table.concat`).
///
/// # Errors
///
/// - `Unsupported`: operazione sconosciuta, non `table.*`, oppure schema di
///   output non inferibile a secco (dipende dai dati);
/// - `Contract`: config non valida, arieta' errata, colonne mancanti,
///   vincoli di tipo o parametri violati, collisioni di naming;
/// - `Schema`: il contratto inferito viola le regole strutturali v1 (D16).
// Un braccio per operazione: la lunghezza e' intrinseca al dispatch su 71 op.
#[allow(clippy::too_many_lines)]
pub fn analyze_table_contract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let descriptor = find_operation(op)
        .ok_or_else(|| PlenoraError::Unsupported(format!("{op}: operazione sconosciuta")))?;
    if descriptor.family != Family::Table {
        return Err(PlenoraError::Unsupported(format!(
            "{op}: non e' un'operazione table.*"
        )));
    }
    let id = descriptor.id;
    match descriptor.arity {
        Arity::Unary if inputs.len() != 1 => {
            return Err(PlenoraError::Contract(format!(
                "{id}: atteso 1 input, ricevuti {}",
                inputs.len()
            )));
        }
        Arity::BinaryOrdered if inputs.len() != 2 => {
            return Err(PlenoraError::Contract(format!(
                "{id}: attesi 2 input (left, right), ricevuti {}",
                inputs.len()
            )));
        }
        Arity::NAry if inputs.len() < 2 => {
            return Err(PlenoraError::Contract(format!(
                "{id}: attesi almeno 2 input, ricevuti {}",
                inputs.len()
            )));
        }
        _ => {}
    }
    for input in inputs {
        for geometry in &input.geometries {
            fields.observe(geometry.field_id);
        }
        if let Some(active) = input.active_geometry {
            fields.observe(active);
        }
        if let Some(sorted) = &input.properties.sorted_by {
            if let Some(keys) = sorted.confidence.value() {
                for key in keys {
                    fields.observe(*key);
                }
            }
        }
    }
    match id {
        "table.add_row_number" => analyze_add_row_number(id, inputs, config, fields),
        "table.aggregate" => analyze_aggregate(id, inputs, config, fields),
        "table.bin" => analyze_bin(id, inputs, config, fields),
        "table.concat" => analyze_concat(id, inputs, config, fields),
        "table.concat_columns" => analyze_concat_columns(id, inputs, config, fields),
        "table.conditional" => analyze_conditional(id, inputs, config, fields),
        "table.cross_join" => analyze_cross_join(id, inputs, config, fields),
        "table.date_extract" => analyze_date_extract(id, inputs, config, fields),
        "table.dedup_advanced" => analyze_dedup_advanced(id, inputs, config, fields),
        "table.distinct" => analyze_distinct(id, inputs, config, fields),
        "table.drop_columns" => analyze_drop_columns(id, inputs, config, fields),
        "table.fill_na" => analyze_fill_na(id, inputs, config, fields),
        "table.filter" => analyze_filter(id, inputs, config, fields),
        "table.flatten_json" => analyze_flatten_json(id, inputs, config, fields),
        "table.formula" => analyze_formula(id, inputs, config, fields),
        "table.join" => analyze_join(id, inputs, config, fields),
        "table.lookup" => analyze_lookup(id, inputs, config, fields),
        "table.melt" => analyze_melt(id, inputs, config, fields),
        "table.pivot" => analyze_pivot(id, inputs, config, fields),
        "table.rename" => analyze_rename(id, inputs, config, fields),
        "table.reorder_columns" => analyze_reorder_columns(id, inputs, config, fields),
        "table.replace" => analyze_replace(id, inputs, config, fields),
        "table.sample" => analyze_sample(id, inputs, config, fields),
        "table.sort" => analyze_sort(id, inputs, config, fields),
        "table.split_column" => analyze_split_column(id, inputs, config, fields),
        "table.statistics" => analyze_statistics(id, inputs, config, fields),
        "table.string_extract" => analyze_string_extract(id, inputs, config, fields),
        "table.string_length" => analyze_string_length(id, inputs, config, fields),
        "table.string_pad" => analyze_string_pad(id, inputs, config, fields),
        "table.table_diff" => analyze_table_diff(id, inputs, config, fields),
        "table.text_normalize" => analyze_text_normalize(id, inputs, config, fields),
        "table.transpose" => analyze_transpose(id, inputs, config, fields),
        "table.type_cast" => analyze_type_cast(id, inputs, config, fields),
        "table.uuid_generator" => analyze_uuid_generator(id, inputs, config, fields),
        "table.window_function" => analyze_window_function(id, inputs, config, fields),
        "table.mask_data" => analyze_mask_data(id, inputs, config, fields),
        "table.md5_hash" => analyze_md5_hash(id, inputs, config, fields),
        "table.anti_join" | "table.semi_join" => {
            analyze_membership_join(id, inputs, config, fields)
        }
        "table.asof_join" => analyze_asof_join(id, inputs, config, fields),
        "table.assert_not_null" => analyze_assert_not_null(id, inputs, config, fields),
        "table.assert_range" => analyze_assert_range(id, inputs, config, fields),
        "table.assert_regex" => analyze_assert_regex(id, inputs, config, fields),
        "table.assert_schema" => analyze_assert_schema(id, inputs, config, fields),
        "table.assert_unique" => analyze_assert_unique(id, inputs, config, fields),
        "table.coalesce" => analyze_coalesce(id, inputs, config, fields),
        "table.date_add" => analyze_date_add(id, inputs, config, fields),
        "table.date_diff" => analyze_date_diff(id, inputs, config, fields),
        "table.date_format" => analyze_date_format(id, inputs, config, fields),
        "table.except" => analyze_set_operation(id, inputs, config, fields, SetOp::Except),
        "table.explode" => analyze_explode(id, inputs, config, fields),
        "table.intersect" => analyze_set_operation(id, inputs, config, fields, SetOp::Intersect),
        "table.rolling_window" => analyze_rolling_window(id, inputs, config, fields),
        "table.sha256_hash" => analyze_sha256_hash(id, inputs, config, fields),
        "table.timezone_convert" => analyze_timezone_convert(id, inputs, config, fields),
        "table.union_distinct" => {
            analyze_set_operation(id, inputs, config, fields, SetOp::UnionDistinct)
        }
        "table.unnest" => analyze_unnest(id, inputs, config, fields),
        "table.expression" => analyze_expression(id, inputs, config, fields),
        "table.assert_cardinality" => analyze_assert_cardinality(id, inputs, config, fields),
        "table.assert_metadata" => analyze_assert_metadata(id, inputs, config, fields),
        "table.assert_foreign_key" => analyze_assert_foreign_key(id, inputs, config, fields),
        "table.reconcile" => analyze_reconcile(id, inputs, config, fields),
        "table.select_columns" => analyze_select_columns(id, inputs, config, fields),
        "table.limit" => analyze_limit(id, inputs, config, fields),
        "table.top_n" => analyze_top_n(id, inputs, config, fields),
        "table.stable_fingerprint" => analyze_stable_fingerprint(id, inputs, config, fields),
        "table.align_schema" => analyze_align_schema(id, inputs, config, fields),
        "table.concat_by_name" => analyze_concat_by_name(id, inputs, config, fields),
        "table.validate_rules" => analyze_validate_rules(id, inputs, config, fields),
        "table.hmac_sha256" => analyze_hmac_sha256(id, inputs, config, fields),
        "table.fuzzy_join" => analyze_fuzzy_join(id, inputs, config, fields),
        _ => Err(PlenoraError::Unsupported(format!(
            "{id}: analyze_contract non disponibile"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Helper condivisi
// ---------------------------------------------------------------------------

/// Deserializza la config tipizzata (fail-closed: `deny_unknown_fields`).
fn typed<T: DeserializeOwned>(op: &str, config: &Value) -> Result<T> {
    serde_json::from_value(config.clone())
        .map_err(|error| PlenoraError::Contract(format!("{op}: config non valida: {error}")))
}

fn contract_error<T>(op: &str, message: impl Into<String>) -> Result<T> {
    Err(PlenoraError::Contract(format!("{op}: {}", message.into())))
}

fn unsupported<T>(op: &str, message: impl Into<String>) -> Result<T> {
    Err(PlenoraError::Unsupported(format!(
        "{op}: {}",
        message.into()
    )))
}

/// Campo per nome, con errore puntuale se assente (replica `column_index`).
fn field_of<'a>(op: &str, input: &'a DataContract, name: &str) -> Result<&'a Field> {
    input
        .schema
        .field_with_name(name)
        .map_err(|_| PlenoraError::Contract(format!("{op}: colonna non trovata: {name}")))
}

/// Tipi leggibili da `scalar_as_string` (profilo scalare testuale).
fn is_scalar_string(data_type: &DataType) -> bool {
    match data_type {
        DataType::Utf8
        | DataType::Int64
        | DataType::Float64
        | DataType::Boolean
        | DataType::UInt64
        | DataType::Date32
        | DataType::Binary
        | DataType::Timestamp(TimeUnit::Millisecond, _)
        | DataType::Decimal128(_, _) => true,
        DataType::Dictionary(key, value) => {
            key.as_ref() == &DataType::Int32 && value.as_ref() == &DataType::Utf8
        }
        _ => false,
    }
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

fn require_scalar_string(op: &str, input: &DataContract, name: &str) -> Result<()> {
    let field = field_of(op, input, name)?;
    if is_scalar_string(field.data_type()) {
        Ok(())
    } else {
        contract_error(
            op,
            format!(
                "colonna {name}: tipo {:?} non leggibile come scalare testuale",
                field.data_type()
            ),
        )
    }
}

fn require_numeric(op: &str, input: &DataContract, name: &str) -> Result<()> {
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

fn require_utf8(op: &str, input: &DataContract, name: &str) -> Result<()> {
    let field = field_of(op, input, name)?;
    if field.data_type() == &DataType::Utf8 {
        Ok(())
    } else {
        contract_error(op, format!("la colonna {name} deve essere Utf8"))
    }
}

fn check_output_name(op: &str, name: &str) -> Result<()> {
    validate_output_name(name)
        .map_err(|_| PlenoraError::Contract(format!("{op}: nome colonna non valido: {name:?}")))
}

/// `round(rows * fraction)` con l'aritmetica f64 del kernel `sample`
/// (i cast riflettono volutamente la sua semantica).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn round_scaled(rows: u64, fraction: f64) -> u64 {
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
fn produce(
    fields: &mut Vec<Field>,
    alloc: &mut FieldAllocator,
    name: &str,
    data_type: DataType,
    nullable: bool,
) -> bool {
    alloc.derive(name);
    upsert(fields, name, data_type, nullable)
}

fn clone_fields(input: &DataContract) -> Vec<Field> {
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
fn propagate_geometry(
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
        nullable: field.is_nullable(),
    })
}

/// Unisce le geometrie dei due rami di un'op binaria: la v1 ammette al
/// massimo una colonna geometrica per arco (D16), due sopravvissute ->
/// errore fail-closed.
fn merge_geometry(
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

fn finish(
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
fn rows_only(input: &DataContract) -> ContractProperties {
    ContractProperties {
        sorted_by: None,
        row_count: input.properties.row_count.clone(),
    }
}

/// Solo `sorted_by` (op che rimuovono righe preservando l'ordine relativo).
fn sorted_only(input: &DataContract) -> ContractProperties {
    ContractProperties {
        sorted_by: input.properties.sorted_by.clone(),
        row_count: None,
    }
}

/// `sorted_by = Proven(chiavi, Stream)`: op blocking che riordina l'intero
/// stream di output (Architetture.md par. 4.3; `execution_class` Blocking).
fn proven_sorted(keys: Vec<FieldId>) -> ContractProperty<Vec<FieldId>> {
    ContractProperty::new(PropertyConfidence::Proven(keys), PropertyScope::Stream)
}

/// Trasforma un `row_count` noto mantenendo provenienza e scope.
fn map_row_count(
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
fn scrub_dropped_geometry(
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
fn analyze_append(
    input: &DataContract,
    alloc: &mut FieldAllocator,
    produced: &[(String, DataType, bool)],
) -> Result<DataContract> {
    let mut fields = clone_fields(input);
    let mut overwritten = false;
    let mut geometry_overwritten = false;
    for (name, data_type, nullable) in produced {
        let replaced = produce(&mut fields, alloc, name, data_type.clone(), *nullable);
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
    let active = input.active_geometry.filter(|id| {
        geometry
            .as_ref()
            .is_some_and(|g| &g.field_id == id)
    });
    finish(schema, geometry, active, append_props(input, overwritten))
}

// ---------------------------------------------------------------------------
// utility.rs
// ---------------------------------------------------------------------------

fn analyze_add_row_number(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::AddRowNumber = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.order_column.is_some() {
        return contract_error(
            op,
            "order_column non supportato dal profilo streaming (deve essere nullo)",
        );
    }
    if let Some(partition) = &config.partition_column {
        require_scalar_string(op, input, partition)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Int64, false)],
    )
}

fn analyze_uuid_generator(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::UuidGenerator = typed(op, config)?;
    check_output_name(op, &config.output_column)?;
    analyze_append(
        &inputs[0],
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

fn analyze_date_extract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::DateExtract = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let mut produced = Vec::with_capacity(config.parts.len());
    for part in &config.parts {
        let suffix = match part {
            utility::DatePart::Year => "year",
            utility::DatePart::Month => "month",
            utility::DatePart::Day => "day",
            utility::DatePart::Quarter => "quarter",
            utility::DatePart::Weekday => "weekday",
            utility::DatePart::Week => "week",
            utility::DatePart::Hour => "hour",
            utility::DatePart::Minute => "minute",
            utility::DatePart::Second => "second",
        };
        let name = format!("{prefix}{suffix}");
        check_output_name(op, &name)?;
        produced.push((name, DataType::Int64, true));
    }
    analyze_append(input, fields, &produced)
}

fn analyze_limit(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: utility::Limit = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let _ = config;
    let mut output = input.clone();
    // Righe rimosse (per-batch), ordine relativo e schema invariati.
    output.properties = sorted_only(input);
    Ok(output)
}

// ---------------------------------------------------------------------------
// strings.rs / security.rs
// ---------------------------------------------------------------------------

fn analyze_string_pad(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::StringPad = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    if config.fill_char.chars().count() != 1 {
        return contract_error(op, "fill_char deve essere un singolo carattere");
    }
    let name = config.output_column.unwrap_or(config.column);
    check_output_name(op, &name)?;
    analyze_append(input, fields, &[(name, DataType::Utf8, true)])
}

fn analyze_string_length(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::StringLength = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    let name = config
        .output_column
        .unwrap_or_else(|| format!("{}_length", config.column));
    check_output_name(op, &name)?;
    analyze_append(input, fields, &[(name, DataType::Int64, true)])
}

fn analyze_string_extract(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::StringExtract = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    if config.pattern.len() > Limits::default().max_regex_bytes {
        return contract_error(op, "pattern oltre max_regex_bytes");
    }
    let pattern = regex::Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::Contract(format!("{op}: regex non valida: {error}")))?;
    let named: Vec<String> = pattern
        .capture_names()
        .flatten()
        .map(str::to_owned)
        .collect();
    let produced: Vec<(String, DataType, bool)> = if named.is_empty() {
        let name = config
            .output_column
            .unwrap_or_else(|| format!("{}_extracted", config.column));
        vec![(name, DataType::Utf8, true)]
    } else {
        // Con gruppi con nome: una colonna per gruppo; output_column ignorato.
        named.into_iter().map(|n| (n, DataType::Utf8, true)).collect()
    };
    for (name, _, _) in &produced {
        check_output_name(op, name)?;
    }
    analyze_append(input, fields, &produced)
}

fn analyze_text_normalize(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: strings::TextNormalize = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut produced = Vec::with_capacity(config.columns.len());
    for name in &config.columns {
        require_utf8(op, input, name)?;
        let output = if config.overwrite {
            name.clone()
        } else {
            format!("{name}_norm")
        };
        check_output_name(op, &output)?;
        produced.push((output, DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

fn analyze_md5_hash(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::Md5Hash = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    for name in &config.columns {
        require_scalar_string(op, input, name)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

fn analyze_sha256_hash(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::Sha256Hash = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    // A differenza di md5_hash il kernel non vieta columns vuoto.
    for name in &config.columns {
        require_scalar_string(op, input, name)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

fn analyze_stable_fingerprint(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::StableFingerprint = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.columns.is_empty() {
        // Default: tutte le colonne dello schema; il kernel legge ogni valore
        // via `scalar_as_string`, quindi i tipi fuori profilo (List, Struct)
        // fallirebbero a runtime: fail-closed gia' in validazione.
        for field in input.schema.fields() {
            if !is_scalar_string(field.data_type()) {
                return contract_error(
                    op,
                    format!(
                        "colonna {}: tipo {:?} non leggibile come scalare testuale",
                        field.name(),
                        field.data_type()
                    ),
                );
            }
        }
    } else {
        let mut seen: HashSet<&str> = HashSet::new();
        for name in &config.columns {
            if !seen.insert(name.as_str()) {
                return contract_error(op, format!("colonna ripetuta in columns: {name}"));
            }
            require_scalar_string(op, input, name)?;
        }
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, false)],
    )
}

fn analyze_hmac_sha256(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::HmacSha256 = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.key_env.trim().is_empty() {
        return contract_error(op, "key_env vuoto");
    }
    // La chiave NON e' mai letta in analisi: nel contratto passa solo il nome
    // della variabile d'ambiente, mai il valore.
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {name}"));
        }
        require_scalar_string(op, input, name)?;
    }
    let nullable = matches!(config.null_policy, security::HmacNullPolicy::Null);
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, nullable)],
    )
}

fn analyze_mask_data(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: security::MaskData = typed(op, config)?;
    let input = &inputs[0];
    if config.maskings.is_empty() {
        return contract_error(op, "maskings vuoto");
    }
    // Le masking sono applicate in sequenza: una masking puo' riferirsi a una
    // colonna `_masked` creata da una precedente.
    let mut available: HashSet<String> = input
        .schema
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();
    let mut produced = Vec::with_capacity(config.maskings.len());
    for masking in &config.maskings {
        if !available.contains(&masking.column) {
            return contract_error(op, format!("colonna non trovata: {}", masking.column));
        }
        require_scalar_string(op, input, &masking.column)?;
        if matches!(masking.mask_type, security::MaskType::Custom)
            && masking.mask_char.chars().count() != 1
        {
            return contract_error(op, "mask_char deve essere un singolo carattere");
        }
        let output = if config.overwrite {
            masking.column.clone()
        } else {
            format!("{}_masked", masking.column)
        };
        check_output_name(op, &output)?;
        available.insert(output.clone());
        produced.push((output, DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

// ---------------------------------------------------------------------------
// dates.rs
// ---------------------------------------------------------------------------

fn analyze_date_op(
    op: &str,
    input: &DataContract,
    fields: &mut FieldAllocator,
    source_columns: &[&str],
    output_column: &str,
    data_type: DataType,
) -> Result<DataContract> {
    for name in source_columns {
        require_scalar_string(op, input, name)?;
    }
    check_output_name(op, output_column)?;
    analyze_append(input, fields, &[(output_column.to_owned(), data_type, true)])
}

fn analyze_date_format(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::DateFormat = typed(op, config)?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.column],
        &config.output_column,
        DataType::Utf8,
    )
}

fn analyze_date_add(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::DateAdd = typed(op, config)?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.column],
        &config.output_column,
        DataType::Utf8,
    )
}

fn analyze_date_diff(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::DateDiff = typed(op, config)?;
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.start_column, &config.end_column],
        &config.output_column,
        DataType::Float64,
    )
}

fn analyze_timezone_convert(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: dates::TimezoneConvert = typed(op, config)?;
    for timezone in [&config.source_timezone, &config.target_timezone] {
        timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| PlenoraError::Contract(format!("{op}: timezone non valida: {timezone}")))?;
    }
    analyze_date_op(
        op,
        &inputs[0],
        fields,
        &[&config.column],
        &config.output_column,
        DataType::Utf8,
    )
}

// ---------------------------------------------------------------------------
// filtering.rs
// ---------------------------------------------------------------------------

/// Replica `json_text` del kernel filtering.
fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Validazioni config-dipendenti di `filter`/`conditional` (valori attesi
/// numerici per confronti ordinati, formato `min,max` di Between).
fn check_operator_value(op: &str, operator: &filtering::Operator, value: &Value) -> Result<()> {
    let expected = json_text(value);
    match operator {
        filtering::Operator::Gt
        | filtering::Operator::Ge
        | filtering::Operator::Lt
        | filtering::Operator::Le => {
            if expected.parse::<f64>().is_err() {
                return contract_error(op, "confronto ordinato richiede un valore numerico");
            }
        }
        filtering::Operator::Between => {
            let Some((low, high)) = expected.split_once(',') else {
                return contract_error(op, "between richiede min,max");
            };
            if low.trim().parse::<f64>().is_err() || high.trim().parse::<f64>().is_err() {
                return contract_error(op, "estremi between non numerici");
            }
        }
        _ => {}
    }
    Ok(())
}

fn analyze_filter(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: filtering::Filter = typed(op, config)?;
    let input = &inputs[0];
    let field = field_of(op, input, &config.column)?;
    // Eq/Ne su colonna numerica con valore non numerico: errore certo a runtime.
    if matches!(config.operator, filtering::Operator::Eq | filtering::Operator::Ne)
        && matches!(field.data_type(), DataType::Int64 | DataType::Float64)
        && json_text(&config.value).parse::<f64>().is_err()
    {
        return contract_error(op, "confronto numerico con valore non numerico");
    }
    check_operator_value(op, &config.operator, &config.value)?;
    let _ = fields;
    // Righe rimosse, ordine relativo e schema invariati.
    let mut output = input.clone();
    output.properties = sorted_only(input);
    Ok(output)
}

fn analyze_conditional(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: filtering::Conditional = typed(op, config)?;
    let input = &inputs[0];
    field_of(op, input, &config.column)?;
    for condition in &config.conditions {
        check_operator_value(op, &condition.operator, &condition.value)?;
    }
    // Il tipo dipende solo dai letterali di config: tutti vuoti o numerici ->
    // Float64 nullable, altrimenti Utf8 non nullable.
    let numeric = config
        .conditions
        .iter()
        .map(|condition| json_text(&condition.result))
        .chain(std::iter::once(json_text(&config.default_value)))
        .all(|text| text.is_empty() || text.replace(',', ".").parse::<f64>().is_ok());
    let (data_type, nullable) = if numeric {
        (DataType::Float64, true)
    } else {
        (DataType::Utf8, false)
    };
    analyze_append(input, fields, &[(config.output_column, data_type, nullable)])
}

// ---------------------------------------------------------------------------
// analysis.rs
// ---------------------------------------------------------------------------

fn analyze_lookup(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Lookup = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    // Default del kernel: sovrascrive la colonna sorgente in place.
    let name = config.output_column.unwrap_or(config.column);
    check_output_name(op, &name)?;
    analyze_append(input, fields, &[(name, DataType::Utf8, true)])
}

fn analyze_bin(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Bin = typed(op, config)?;
    let input = &inputs[0];
    require_numeric(op, input, &config.column)?;
    let bins = match &config.bins {
        analysis::Bins::Count(count) => {
            if !(2..=100).contains(count) {
                return contract_error(op, "bins count fuori da 2..=100");
            }
            *count
        }
        analysis::Bins::Edges(edges) => {
            if !(3..=101).contains(&edges.len()) {
                return contract_error(op, "edges fuori da 3..=101");
            }
            if edges.windows(2).any(|pair| pair[0] >= pair[1]) {
                return contract_error(op, "edges non strettamente crescenti");
            }
            edges.len() - 1
        }
    };
    if let Some(labels) = &config.labels {
        if labels.len() != bins {
            return contract_error(
                op,
                format!("labels ({}) diversi dai bin ({bins})", labels.len()),
            );
        }
    }
    let name = config
        .output_column
        .unwrap_or_else(|| format!("{}_bin", config.column));
    analyze_append(input, fields, &[(name, DataType::Utf8, true)])
}

fn analyze_flatten_json(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::FlattenJson = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    if config.max_level > 5 {
        return contract_error(op, "max_level oltre 5");
    }
    if config.output_columns.is_empty() {
        return unsupported(
            op,
            "senza output_columns i nomi delle colonne derivano dai dati: schema non inferibile a secco",
        );
    }
    let prefix = if config.prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.prefix.clone()
    };
    let mut produced = Vec::with_capacity(config.output_columns.len());
    for name in &config.output_columns {
        if !name.starts_with(&prefix) {
            return contract_error(
                op,
                format!("output column {name:?} non inizia con il prefix {prefix:?}"),
            );
        }
        check_output_name(op, name)?;
        produced.push((name.clone(), DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

fn analyze_statistics(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Statistics = typed(op, config)?;
    let input = &inputs[0];
    require_numeric(op, input, &config.column)?;
    if let Some(group_by) = &config.group_by {
        field_of(op, input, group_by)?;
    }
    let prefix = if config.output_prefix.is_empty() {
        format!("{}_", config.column)
    } else {
        config.output_prefix.clone()
    };
    let produced: Vec<(String, DataType, bool)> = config
        .stats
        .iter()
        .map(|stat| {
            let suffix = match stat {
                analysis::Stat::Count => "count",
                analysis::Stat::Min => "min",
                analysis::Stat::Max => "max",
                analysis::Stat::Sum => "sum",
                analysis::Stat::Mean => "mean",
                analysis::Stat::Median => "median",
                analysis::Stat::Std => "std",
                analysis::Stat::Var => "var",
                analysis::Stat::Q25 => "q25",
                analysis::Stat::Q75 => "q75",
            };
            (format!("{prefix}{suffix}"), DataType::Float64, true)
        })
        .collect();
    // Statistiche broadcast per riga: righe invariate.
    analyze_append(input, fields, &produced)
}

fn analyze_sample(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: analysis::Sample = typed(op, config)?;
    let input = &inputs[0];
    if config
        .fraction
        .is_some_and(|fraction| !(0.0..=1.0).contains(&fraction))
    {
        return contract_error(op, "fraction fuori 0..=1");
    }
    if let Some(stratify) = &config.stratify_column {
        require_scalar_string(op, input, stratify)?;
    }
    let _ = fields;
    let mut output = input.clone();
    // Il kernel mescola le righe (shuffle): nessun ordinamento preservato.
    // Senza stratify il conteggio e' esatto: min(n, righe) o round(righe*f)
    // (stessa aritmetica f64 del kernel).
    let row_count = if config.stratify_column.is_none() {
        map_row_count(input, |rows| match config.fraction {
            None => rows.min(u64::try_from(config.n).unwrap_or(u64::MAX)),
            Some(fraction) => round_scaled(rows, fraction),
        })
    } else {
        None
    };
    output.properties = ContractProperties {
        sorted_by: None,
        row_count,
    };
    Ok(output)
}

// ---------------------------------------------------------------------------
// columns.rs
// ---------------------------------------------------------------------------

fn analyze_drop_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::DropColumns = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let to_drop: HashSet<&str> = config.columns.iter().map(String::as_str).collect();
    let kept: Vec<Field> = clone_fields(input)
        .into_iter()
        .filter(|field| !to_drop.contains(field.name().as_str()))
        .collect();
    let removed_any = kept.len() != input.schema.fields().len();
    let schema = Schema::new_with_metadata(kept, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let dropped_geometry = if geometry.is_none() {
        input.geometries.first().map(|g| g.field_id)
    } else {
        None
    };
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    // Conservativo: una colonna rimossa puo' essere una chiave di ordinamento.
    let properties = scrub_dropped_geometry(
        ContractProperties {
            sorted_by: if removed_any {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
        dropped_geometry,
    );
    finish(schema, geometry, active, properties)
}

fn analyze_rename(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::Rename = typed(op, config)?;
    let input = &inputs[0];
    // Come il kernel: old_name inesistente ignorato, duplicati -> vince l'ultimo.
    let renames: HashMap<&str, &str> = config
        .renames
        .iter()
        .map(|pair| (pair.old_name.as_str(), pair.new_name.as_str()))
        .collect();
    let mut fields_out = Vec::with_capacity(input.schema.fields().len());
    let mut names: HashSet<String> = HashSet::new();
    for field in clone_fields(input) {
        let name = renames
            .get(field.name().as_str())
            .copied()
            .unwrap_or_else(|| field.name().as_str())
            .to_owned();
        check_output_name(op, &name)?;
        if !names.insert(name.clone()) {
            return contract_error(op, format!("rename produce il nome duplicato: {name}"));
        }
        fields_out.push(field.with_name(name));
    }
    for pair in &config.renames {
        fields.rename(&pair.old_name, &pair.new_name);
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let renamed_geometry = input.geometries.first().map(|geometry| {
        renames
            .get(geometry.name.as_str())
            .copied()
            .unwrap_or(geometry.name.as_str())
    });
    let geometry = propagate_geometry(input, &schema, renamed_geometry);
    // Rinomina: FieldId preservati (D16), righe e ordine invariati.
    finish(schema, geometry, input.active_geometry, input.properties.clone())
}

fn analyze_reorder_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::ReorderColumns = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let mut seen: HashSet<&str> = HashSet::new();
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {name}"));
        }
        field_of(op, input, name)?;
    }
    let mut ordered: Vec<Field> = config
        .columns
        .iter()
        .map(|name| field_of(op, input, name).cloned())
        .collect::<Result<_>>()?;
    let mut rest: Vec<Field> = clone_fields(input)
        .into_iter()
        .filter(|field| !seen.contains(field.name().as_str()))
        .collect();
    if config.alphabetical {
        rest.sort_by_key(|field| field.name().to_lowercase());
    }
    ordered.extend(rest);
    let schema = Schema::new_with_metadata(ordered, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    finish(schema, geometry, input.active_geometry, input.properties.clone())
}

fn analyze_concat_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::ConcatColumns = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    check_output_name(op, &config.output_column)?;
    for name in &config.columns {
        require_utf8(op, input, name)?;
    }
    analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Utf8, true)],
    )
}

fn analyze_split_column(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::SplitColumn = typed(op, config)?;
    let input = &inputs[0];
    if config.delimiter.is_empty() {
        return contract_error(op, "delimiter vuoto");
    }
    if config.new_columns.is_empty() {
        return contract_error(op, "new_columns vuoto");
    }
    if config.new_columns.len() > Limits::default().max_split_columns {
        return contract_error(op, "new_columns oltre max_split_columns");
    }
    require_utf8(op, input, &config.column)?;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut produced = Vec::with_capacity(config.new_columns.len());
    for name in &config.new_columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("new_columns duplicato: {name}"));
        }
        check_output_name(op, name)?;
        produced.push((name.clone(), DataType::Utf8, true));
    }
    analyze_append(input, fields, &produced)
}

fn analyze_select_columns(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::SelectColumns = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut fields_out = Vec::with_capacity(config.columns.len());
    for name in &config.columns {
        if !seen.insert(name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {name}"));
        }
        fields_out.push(field_of(op, input, name)?.clone());
    }
    let removed_any = fields_out.len() != input.schema.fields().len();
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let dropped_geometry = if geometry.is_none() {
        input.geometries.first().map(|g| g.field_id)
    } else {
        None
    };
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    // Come drop_columns: una colonna rimossa puo' essere chiave di ordinamento.
    let properties = scrub_dropped_geometry(
        ContractProperties {
            sorted_by: if removed_any {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
        dropped_geometry,
    );
    finish(schema, geometry, active, properties)
}

fn analyze_align_schema(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: columns::AlignSchema = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut fields_out = Vec::with_capacity(config.columns.len());
    let mut added_any = false;
    for declared in &config.columns {
        check_output_name(op, &declared.name)?;
        if !seen.insert(declared.name.as_str()) {
            return contract_error(op, format!("colonna ripetuta in columns: {}", declared.name));
        }
        let data_type = declared.align_type.data_type();
        if let Ok(field) = input.schema.field_with_name(&declared.name) {
            // Mai cast implicito: il tipo dichiarato deve essere identico.
            if field.data_type() != &data_type {
                return contract_error(
                    op,
                    format!(
                        "colonna {} di tipo {:?}, atteso {:?} (nessun cast implicito)",
                        declared.name,
                        field.data_type(),
                        data_type
                    ),
                );
            }
            fields_out.push(field.clone());
        } else {
            added_any = true;
            // Il default e' validato a secco con la stessa conversione del
            // kernel: mai un errore a meta' dei dati.
            if let Some(default) = &declared.default {
                columns::check_align_default(default, declared.align_type)?;
            }
            fields.derive(&declared.name);
            fields_out.push(Field::new(
                &declared.name,
                data_type,
                declared.default.is_none(),
            ));
        }
    }
    let removed_any = !config.keep_extra
        && input
            .schema
            .fields()
            .iter()
            .any(|field| !seen.contains(field.name().as_str()));
    if config.keep_extra {
        for field in input.schema.fields() {
            if !seen.contains(field.name().as_str()) {
                fields_out.push(field.as_ref().clone());
            }
        }
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry =
        propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let dropped_geometry = if geometry.is_none() {
        input.geometries.first().map(|g| g.field_id)
    } else {
        None
    };
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    // 1:1 sulle righe: row_count invariato. sorted_by solo se l'insieme delle
    // colonne e' invariato (permutazione pura, nessuna chiave rimossa).
    let properties = scrub_dropped_geometry(
        ContractProperties {
            sorted_by: if added_any || removed_any {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
        dropped_geometry,
    );
    finish(schema, geometry, active, properties)
}

// ---------------------------------------------------------------------------
// cleansing.rs
// ---------------------------------------------------------------------------

/// Coerenza valore-tipo di `fill_na` (il kernel calcola il valore fisso per
/// ogni metodo, quindi l'errore e' deterministico a secco).
fn check_fill_value(op: &str, data_type: &DataType, value: &Value) -> Result<()> {
    let valid = match data_type {
        DataType::Int64 => match value {
            Value::Null => true,
            Value::Number(number) => number.as_i64().is_some(),
            Value::String(text) => text.parse::<i64>().is_ok(),
            _ => false,
        },
        DataType::Float64 => match value {
            Value::Null => true,
            Value::Number(number) => number.as_f64().is_some(),
            Value::String(text) => text.replace(',', ".").parse::<f64>().is_ok(),
            _ => false,
        },
        DataType::Boolean => match value {
            Value::Null | Value::Bool(_) => true,
            Value::String(text) => {
                text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false")
            }
            _ => false,
        },
        DataType::Utf8 => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        contract_error(
            op,
            format!("valore di fill non valido per il tipo {data_type:?}"),
        )
    }
}

fn analyze_fill_na(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: cleansing::FillNa = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let targets: Vec<usize> = if let Some(name) = &config.column {
        vec![input
            .schema
            .index_of(name)
            .map_err(|_| PlenoraError::Contract(format!("{op}: colonna non trovata: {name}")))?]
    } else {
        (0..input.schema.fields().len()).collect()
    };
    let mut fields_out = clone_fields(input);
    for index in targets {
        let data_type = fields_out[index].data_type().clone();
        if !matches!(
            data_type,
            DataType::Utf8 | DataType::Int64 | DataType::Float64 | DataType::Boolean
        ) {
            return contract_error(
                op,
                format!(
                    "fill_na non supporta il tipo {data_type:?} della colonna {}",
                    fields_out[index].name()
                ),
            );
        }
        check_fill_value(op, &data_type, &config.value)?;
        fields_out[index] = Field::new(fields_out[index].name(), data_type, true);
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    // La colonna geometrica (Binary) non e' mai un target valido: preservata.
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    // Valori modificati, righe e ordine invariati.
    finish(schema, geometry, input.active_geometry, rows_only(input))
}

fn analyze_replace(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: cleansing::Replace = typed(op, config)?;
    let input = &inputs[0];
    require_utf8(op, input, &config.column)?;
    if config.regex {
        regex::Regex::new(&config.old_value)
            .map_err(|error| PlenoraError::Contract(format!("{op}: regex non valida: {error}")))?;
    }
    let mut fields_out = clone_fields(input);
    produce(&mut fields_out, fields, &config.column, DataType::Utf8, true);
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    finish(schema, geometry, input.active_geometry, rows_only(input))
}

fn analyze_type_cast(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: cleansing::TypeCast = typed(op, config)?;
    let input = &inputs[0];
    require_scalar_string(op, input, &config.column)?;
    let target = match config.target_type {
        cleansing::TargetType::Str | cleansing::TargetType::Date | cleansing::TargetType::Datetime => {
            DataType::Utf8
        }
        cleansing::TargetType::Int => DataType::Int64,
        cleansing::TargetType::Float => DataType::Float64,
        cleansing::TargetType::Bool => DataType::Boolean,
        cleansing::TargetType::Date32 => DataType::Date32,
        cleansing::TargetType::TimestampMillis => {
            if let Some(timezone) = &config.timezone {
                timezone.parse::<chrono_tz::Tz>().map_err(|_| {
                    PlenoraError::Contract(format!("{op}: timezone non valida: {timezone}"))
                })?;
            }
            DataType::Timestamp(
                TimeUnit::Millisecond,
                config.timezone.as_deref().map(Into::into),
            )
        }
        cleansing::TargetType::Decimal128 => {
            let precision = config
                .precision
                .ok_or_else(|| PlenoraError::Contract(format!("{op}: decimal128 richiede precision")))?;
            let scale = config
                .scale
                .ok_or_else(|| PlenoraError::Contract(format!("{op}: decimal128 richiede scale")))?;
            if precision == 0 || precision > 38 {
                return contract_error(op, "precision decimal128 fuori da 1..=38");
            }
            DataType::Decimal128(precision, scale)
        }
        cleansing::TargetType::BinaryUtf8 => DataType::Binary,
        cleansing::TargetType::Uint64 => DataType::UInt64,
        cleansing::TargetType::DictionaryUtf8 => DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        ),
    };
    // Sostituzione in place: i metadati di campo (geoarrow.wkb inclusi) vanno
    // persi -> se il target e' la colonna geometrica il contratto diventa
    // tabellare (analyze_append lo gestisce). errors=Ignore puo' fallire a
    // runtime su dati non omogenei: rischio documentato, non errore statico.
    let mut output = analyze_append(input, fields, &[(config.column, target, true)])?;
    output.properties = rows_only(input);
    Ok(output)
}

// ---------------------------------------------------------------------------
// aggregation.rs
// ---------------------------------------------------------------------------

fn analyze_sort(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::Sort = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    for name in &config.columns {
        field_of(op, input, name)?;
    }
    let keys: Vec<FieldId> = config.columns.iter().map(|name| fields.intern(name)).collect();
    let mut output = input.clone();
    // Sort blocking: l'intero stream di output e' ordinato sulle chiavi.
    output.properties = ContractProperties {
        sorted_by: Some(proven_sorted(keys)),
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

fn analyze_top_n(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::TopN = typed(op, config)?;
    let input = &inputs[0];
    if config.columns.is_empty() {
        return contract_error(op, "columns vuoto");
    }
    for name in &config.columns {
        field_of(op, input, name)?;
    }
    let keys: Vec<FieldId> = config.columns.iter().map(|name| fields.intern(name)).collect();
    let mut output = input.clone();
    // Come sort, ma emesse esattamente min(n, righe) righe.
    output.properties = ContractProperties {
        sorted_by: Some(proven_sorted(keys)),
        row_count: map_row_count(input, |rows| rows.min(config.n)),
    };
    Ok(output)
}

fn analyze_distinct(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::Distinct = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in &config.subset {
        require_scalar_string(op, input, name)?;
    }
    let mut output = input.clone();
    // Righe rimosse; l'ordine relativo delle occorrenze mantenute e' preservato.
    output.properties = sorted_only(input);
    Ok(output)
}

fn analyze_dedup_advanced(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::DedupAdvanced = typed(op, config)?;
    let input = &inputs[0];
    if matches!(config.keep, aggregation::Keep::False) {
        return contract_error(op, "keep=false non supportato");
    }
    for name in &config.subset {
        require_scalar_string(op, input, name)?;
    }
    let sorted_by = if let Some(order_column) = &config.order_column {
        field_of(op, input, order_column)?;
        // Sort interno ascendente su order_column prima della deduplica.
        Some(proven_sorted(vec![fields.intern(order_column)]))
    } else {
        input.properties.sorted_by.clone()
    };
    let mut output = input.clone();
    output.properties = ContractProperties {
        sorted_by,
        row_count: None,
    };
    Ok(output)
}

fn analyze_aggregate(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::Aggregate = typed(op, config)?;
    let input = &inputs[0];
    if config.group_by.is_empty() {
        return contract_error(op, "aggregate richiede group_by");
    }
    let mut fields_out: Vec<Field> = config
        .group_by
        .iter()
        .map(|name| field_of(op, input, name).cloned())
        .collect::<Result<_>>()?;
    let mut duplicates: HashMap<&str, usize> = HashMap::new();
    for aggregation in &config.aggregations {
        *duplicates.entry(aggregation.column.as_str()).or_insert(0) += 1;
    }
    for aggregation in &config.aggregations {
        let field = field_of(op, input, &aggregation.column)?;
        match aggregation.function {
            aggregation::AggFunction::Count => {}
            aggregation::AggFunction::Nunique
            | aggregation::AggFunction::Concat
            | aggregation::AggFunction::First
            | aggregation::AggFunction::Last => {
                if !is_scalar_string(field.data_type()) {
                    return contract_error(
                        op,
                        format!("colonna {} non aggregabile come testo", aggregation.column),
                    );
                }
            }
            aggregation::AggFunction::Quantile => {
                if aggregation.quantile.is_none() {
                    return contract_error(op, "quantile richiede il parametro quantile");
                }
                require_numeric(op, input, &aggregation.column)?;
            }
            _ => require_numeric(op, input, &aggregation.column)?,
        }
        let function_name = match aggregation.function {
            aggregation::AggFunction::Count => "count",
            aggregation::AggFunction::Sum => "sum",
            aggregation::AggFunction::Avg | aggregation::AggFunction::Mean => "mean",
            aggregation::AggFunction::Min => "min",
            aggregation::AggFunction::Max => "max",
            aggregation::AggFunction::First => "first",
            aggregation::AggFunction::Last => "last",
            aggregation::AggFunction::Concat => "concat",
            aggregation::AggFunction::Nunique => "nunique",
            aggregation::AggFunction::Variance => "variance",
            aggregation::AggFunction::Stddev => "stddev",
            aggregation::AggFunction::Quantile => "quantile",
        };
        let name = if !aggregation.alias.is_empty() {
            aggregation.alias.clone()
        } else if duplicates[aggregation.column.as_str()] > 1 {
            format!("{}_{function_name}", aggregation.column)
        } else {
            aggregation.column.clone()
        };
        check_output_name(op, &name)?;
        let (data_type, nullable) = match aggregation.function {
            aggregation::AggFunction::Count | aggregation::AggFunction::Nunique => {
                (DataType::Int64, false)
            }
            aggregation::AggFunction::Concat
            | aggregation::AggFunction::First
            | aggregation::AggFunction::Last => (DataType::Utf8, true),
            _ => (DataType::Float64, true),
        };
        produce(&mut fields_out, fields, &name, data_type, nullable);
    }
    if config.aggregations.is_empty() {
        produce(&mut fields_out, fields, "count", DataType::Int64, false);
    }
    // Base group_by costruita con Schema::new: metadata non preservati.
    let schema = Schema::new(fields_out);
    let preserved = input
        .geometries
        .first()
        .filter(|geometry| config.group_by.contains(&geometry.name))
        .map(|geometry| geometry.name.as_str());
    let geometry = propagate_geometry(input, &schema, preserved);
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(schema, geometry, active, ContractProperties::default())
}

fn analyze_rolling_window(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::RollingWindow = typed(op, config)?;
    let input = &inputs[0];
    if config.window == 0 || config.min_periods == 0 || config.min_periods > config.window {
        return contract_error(op, "window/min_periods non validi");
    }
    require_numeric(op, input, &config.column)?;
    if let Some(group_by) = &config.group_by {
        field_of(op, input, group_by)?;
    }
    let sorted_by = if let Some(order_column) = &config.order_column {
        field_of(op, input, order_column)?;
        Some(proven_sorted(vec![fields.intern(order_column)]))
    } else {
        input.properties.sorted_by.clone()
    };
    let mut output = analyze_append(
        input,
        fields,
        &[(config.output_column, DataType::Float64, true)],
    )?;
    output.properties = ContractProperties {
        sorted_by,
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

fn analyze_window_function(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: aggregation::WindowFunction = typed(op, config)?;
    let input = &inputs[0];
    if config.offset == 0 {
        return contract_error(op, "offset deve essere > 0");
    }
    match (&config.function, config.buckets) {
        (aggregation::WindowKind::Ntile, Some(buckets)) if buckets > 0 => {}
        (aggregation::WindowKind::Ntile, _) => {
            return contract_error(op, "ntile richiede buckets > 0");
        }
        (_, Some(_)) => {
            return contract_error(op, "buckets ammesso solo con ntile");
        }
        _ => {}
    }
    require_numeric(op, input, &config.column)?;
    if let Some(group_by) = &config.group_by {
        field_of(op, input, group_by)?;
    }
    if let Some(order_column) = &config.order_column {
        field_of(op, input, order_column)?;
    }
    let suffix = match config.function {
        aggregation::WindowKind::Rank => "rank",
        aggregation::WindowKind::DenseRank => "dense_rank",
        aggregation::WindowKind::Cumsum => "cumsum",
        aggregation::WindowKind::Cumcount => "cumcount",
        aggregation::WindowKind::Lag => "lag",
        aggregation::WindowKind::Lead => "lead",
        aggregation::WindowKind::PctChange => "pct_change",
        aggregation::WindowKind::RunningMean => "running_mean",
        aggregation::WindowKind::PercentRank => "percent_rank",
        aggregation::WindowKind::CumeDist => "cume_dist",
        aggregation::WindowKind::Ntile => "ntile",
    };
    let name = config
        .output_column
        .unwrap_or_else(|| format!("{}_{suffix}", config.column));
    let sorted_by = config.order_column.as_ref().map_or_else(
        || input.properties.sorted_by.clone(),
        |order_column| Some(proven_sorted(vec![fields.intern(order_column)])),
    );
    let mut output = analyze_append(input, fields, &[(name, DataType::Float64, true)])?;
    output.properties = ContractProperties {
        sorted_by,
        row_count: input.properties.row_count.clone(),
    };
    Ok(output)
}

// ---------------------------------------------------------------------------
// formula.rs / expressions.rs
// ---------------------------------------------------------------------------

fn analyze_formula(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: formula::Formula = typed(op, config)?;
    let input = &inputs[0];
    formula::validate(&config, Limits::default().max_string_bytes)
        .map_err(|error| PlenoraError::Contract(format!("{op}: {error}")))?;
    let inferred = formula::infer_formula_type(&config, &|name| {
        let field = field_of(op, input, name)?;
        if matches!(field.data_type(), DataType::Int64 | DataType::Float64) {
            Ok(formula::FormulaType::Number)
        } else if is_scalar_string(field.data_type()) {
            Ok(formula::FormulaType::Text)
        } else {
            contract_error(
                op,
                format!("colonna {name}: tipo {:?} non valutabile", field.data_type()),
            )
        }
    })?;
    let data_type = match inferred {
        formula::FormulaType::Number => DataType::Float64,
        formula::FormulaType::Text => DataType::Utf8,
    };
    analyze_append(input, fields, &[(config.new_column, data_type, true)])
}

/// Tipo statico di un sotto-albero `Expression` (`Any` = letterale null,
/// tipo deciso dai dati ma coerente con qualsiasi altro). `Date32` e
/// `TimestampMs` sono i tipi temporali NATIVI prodotti da `date_trunc`
/// (decisione registrata: nessuna degradazione a Number per l'output di
/// `date_trunc`; le colonne Date32/Timestamp lette direttamente restano
/// `Number`, come nel kernel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StaticType {
    Any,
    Number,
    Boolean,
    Text,
    Date32,
    TimestampMs,
}

fn meet_types(op: &str, left: StaticType, right: StaticType) -> Result<StaticType> {
    match (left, right) {
        (StaticType::Any, other) | (other, StaticType::Any) => Ok(other),
        (left, right) if left == right => Ok(left),
        _ => contract_error(
            op,
            "tipi eterogenei nell'espressione: dichiarare output_type esplicito",
        ),
    }
}

fn expect_type(
    op: &str,
    actual: StaticType,
    expected: StaticType,
    context: &str,
) -> Result<StaticType> {
    if actual == StaticType::Any || actual == expected {
        Ok(expected)
    } else {
        contract_error(op, format!("{context} richiede un operando {expected:?}"))
    }
}

// Le regole di tipo dell'AST sono intrinsecamente ramificate.
#[allow(clippy::too_many_lines)]
fn infer_expression_type(
    op: &str,
    input: &DataContract,
    expression: &expressions::Expression,
) -> Result<StaticType> {
    use expressions::{BinaryOperator, Expression, Function, UnaryOperator};
    match expression {
        Expression::Column { name } => {
            let field = field_of(op, input, name)?;
            Ok(match field.data_type() {
                DataType::Boolean => StaticType::Boolean,
                DataType::Int64
                | DataType::UInt64
                | DataType::Float64
                | DataType::Decimal128(_, _)
                | DataType::Date32
                | DataType::Timestamp(_, _) => StaticType::Number,
                data_type if is_scalar_string(data_type) => StaticType::Text,
                data_type => {
                    return contract_error(
                        op,
                        format!("colonna {name}: tipo {data_type:?} non valutabile"),
                    );
                }
            })
        }
        Expression::Literal { value } => match value {
            Value::Null => Ok(StaticType::Any),
            Value::Bool(_) => Ok(StaticType::Boolean),
            Value::Number(_) => Ok(StaticType::Number),
            Value::String(_) => Ok(StaticType::Text),
            Value::Array(_) | Value::Object(_) => {
                contract_error(op, "literal expression deve essere scalare")
            }
        },
        Expression::Unary { op: operator, value } => {
            let operand = infer_expression_type(op, input, value)?;
            match operator {
                UnaryOperator::Not => expect_type(op, operand, StaticType::Boolean, "not"),
                UnaryOperator::Negate => expect_type(op, operand, StaticType::Number, "negate"),
                UnaryOperator::IsNull | UnaryOperator::IsNotNull => Ok(StaticType::Boolean),
            }
        }
        Expression::Binary {
            op: operator,
            left,
            right,
        } => {
            let left = infer_expression_type(op, input, left)?;
            let right = infer_expression_type(op, input, right)?;
            match operator {
                BinaryOperator::Add
                | BinaryOperator::Subtract
                | BinaryOperator::Multiply
                | BinaryOperator::Divide => {
                    expect_type(op, left, StaticType::Number, "operatore aritmetico")?;
                    expect_type(op, right, StaticType::Number, "operatore aritmetico")
                }
                BinaryOperator::And | BinaryOperator::Or => {
                    expect_type(op, left, StaticType::Boolean, "operatore logico")?;
                    expect_type(op, right, StaticType::Boolean, "operatore logico")
                }
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual => {
                    meet_types(op, left, right)?;
                    Ok(StaticType::Boolean)
                }
            }
        }
        Expression::Function { name, args } => {
            // Nodi speciali: la lista di `in` non e' uno scalare valutabile
            // e `date_trunc` ha regole di tipo temporali native dedicate.
            if matches!(name, Function::DateTrunc) {
                return infer_date_trunc_type(op, input, args);
            }
            if matches!(name, Function::In) {
                if args.len() != 2 {
                    return contract_error(op, "in richiede 2 argomenti");
                }
                infer_expression_type(op, input, &args[0])?;
                return Ok(StaticType::Boolean);
            }
            let types = args
                .iter()
                .map(|argument| infer_expression_type(op, input, argument))
                .collect::<Result<Vec<_>>>()?;
            let fold = |types: &[StaticType]| {
                types.iter().try_fold(StaticType::Any, |acc, item| {
                    meet_types(op, acc, *item)
                })
            };
            match name {
                Function::Coalesce | Function::NullIf => fold(&types),
                Function::Lower | Function::Upper | Function::Trim => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "funzione testuale")?;
                    }
                    Ok(StaticType::Text)
                }
                Function::Concat => Ok(StaticType::Text),
                Function::Length => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "length")?;
                    }
                    Ok(StaticType::Number)
                }
                Function::Contains | Function::StartsWith | Function::EndsWith => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "predicato testuale")?;
                    }
                    Ok(StaticType::Boolean)
                }
                Function::Abs | Function::Round | Function::Year => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Number, "funzione numerica")?;
                    }
                    Ok(StaticType::Number)
                }
                Function::Substring => {
                    // (testo, numero, numero?) -> testo
                    if let Some((first, rest)) = types.split_first() {
                        expect_type(op, *first, StaticType::Text, "substring")?;
                        for item in rest {
                            expect_type(op, *item, StaticType::Number, "substring")?;
                        }
                    }
                    Ok(StaticType::Text)
                }
                Function::RegexReplace => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Text, "regex_replace")?;
                    }
                    Ok(StaticType::Text)
                }
                Function::Between => {
                    // Omogeneita' degli operandi come i confronti binari.
                    fold(&types)?;
                    Ok(StaticType::Boolean)
                }
                Function::Greatest | Function::Least => fold(&types),
                Function::Floor | Function::Ceil | Function::Power => {
                    for item in &types {
                        expect_type(op, *item, StaticType::Number, "funzione numerica")?;
                    }
                    Ok(StaticType::Number)
                }
                Function::DateTrunc | Function::In => unreachable!(),
            }
        }
        Expression::Case {
            branches,
            else_value,
        } => {
            let mut result = infer_expression_type(op, input, else_value)?;
            for branch in branches {
                infer_expression_type(op, input, &branch.when)?;
                result = meet_types(op, result, infer_expression_type(op, input, &branch.then)?)?;
            }
            Ok(result)
        }
    }
}

fn analyze_expression(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: expressions::ExpressionTransform = typed(op, config)?;
    let input = &inputs[0];
    expressions::validate(&config, MAX_EXPRESSION_NODES)
        .map_err(|error| PlenoraError::Contract(format!("{op}: {error}")))?;
    check_output_name(op, &config.output_column)?;
    let data_type = match config.output_type {
        expressions::OutputType::Number => DataType::Float64,
        expressions::OutputType::Boolean => DataType::Boolean,
        expressions::OutputType::Text => DataType::Utf8,
        expressions::OutputType::Date32 => DataType::Date32,
        expressions::OutputType::TimestampMs => DataType::Timestamp(TimeUnit::Millisecond, None),
        expressions::OutputType::Auto => {
            // Auto e' risolto dal kernel sui dati: l'analisi statica prova a
            // determinarlo dall'AST; tutto-null -> Text come il kernel, salvo
            // radice date_trunc (tipo temporale dalla colonna di input).
            match infer_expression_type(op, input, &config.expression)? {
                StaticType::Number => DataType::Float64,
                StaticType::Boolean => DataType::Boolean,
                StaticType::Any | StaticType::Text => DataType::Utf8,
                StaticType::Date32 => DataType::Date32,
                StaticType::TimestampMs => DataType::Timestamp(TimeUnit::Millisecond, None),
            }
        }
    };
    analyze_append(input, fields, &[(config.output_column, data_type, true)])
}

/// Regole di tipo di `date_trunc` (tipi temporali nativi, decisione
/// registrata): l'unita' e' un letterale del set chiuso; il tipo di output
/// discende dal tipo della colonna di input (anche su dati tutti null);
/// nessun parsing implicito di stringhe; timestamp timezone-aware rifiutati
/// (semantica tz del troncamento non definibile in modo sicuro: l'output e'
/// sempre naive).
fn infer_date_trunc_type(
    op: &str,
    input: &DataContract,
    args: &[expressions::Expression],
) -> Result<StaticType> {
    if args.len() != 2 {
        return contract_error(op, "date_trunc richiede 2 argomenti");
    }
    let expressions::Expression::Literal {
        value: Value::String(unit),
    } = &args[0]
    else {
        return contract_error(op, "date_trunc: unit deve essere un letterale stringa");
    };
    if !matches!(
        unit.as_str(),
        "year" | "month" | "day" | "hour" | "minute" | "second"
    ) {
        return contract_error(op, format!("date_trunc: unita' non valida: {unit}"));
    }
    temporal_static_type(op, input, &args[1], unit)
}

/// Tipo temporale statico della sorgente di `date_trunc`; `unit` e' l'unita'
/// del livello corrente (sub-day rifiutata su Date32).
fn temporal_static_type(
    op: &str,
    input: &DataContract,
    expression: &expressions::Expression,
    unit: &str,
) -> Result<StaticType> {
    use expressions::Expression;
    match expression {
        Expression::Column { name } => {
            let field = field_of(op, input, name)?;
            match field.data_type() {
                DataType::Date32 => {
                    if matches!(unit, "hour" | "minute" | "second") {
                        return contract_error(
                            op,
                            "date_trunc: unita' sub-day non ammessa su Date32",
                        );
                    }
                    Ok(StaticType::Date32)
                }
                DataType::Timestamp(TimeUnit::Millisecond, timezone) => {
                    if timezone.is_some() {
                        return contract_error(
                            op,
                            "date_trunc: timestamp timezone-aware non supportato",
                        );
                    }
                    Ok(StaticType::TimestampMs)
                }
                other => contract_error(
                    op,
                    format!(
                        "date_trunc richiede una colonna Date32 o Timestamp(ms), trovato {other:?}"
                    ),
                ),
            }
        }
        Expression::Function {
            name: expressions::Function::DateTrunc,
            args,
        } => {
            let kind = infer_date_trunc_type(op, input, args)?;
            if kind == StaticType::Date32 && matches!(unit, "hour" | "minute" | "second") {
                return contract_error(op, "date_trunc: unita' sub-day non ammessa su Date32");
            }
            Ok(kind)
        }
        Expression::Literal {
            value: Value::Null,
        } => Ok(StaticType::Any),
        _ => contract_error(op, "date_trunc: il valore deve essere una colonna temporale"),
    }
}

// ---------------------------------------------------------------------------
// quality.rs
// ---------------------------------------------------------------------------

fn analyze_assert_schema(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertSchema = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    if !config.allow_extra && input.schema.fields().len() != config.fields.len() {
        return contract_error(
            op,
            format!(
                "attese {} colonne, trovate {}",
                config.fields.len(),
                input.schema.fields().len()
            ),
        );
    }
    for (position, expectation) in config.fields.iter().enumerate() {
        let field = if config.ordered {
            input.schema.fields().get(position).ok_or_else(|| {
                PlenoraError::Contract(format!(
                    "{op}: colonna mancante in posizione {position}"
                ))
            })?
        } else {
            input
                .schema
                .field_with_name(&expectation.name)
                .map_err(|_| {
                    PlenoraError::Contract(format!(
                        "{op}: colonna mancante {}",
                        expectation.name
                    ))
                })?
        };
        if field.name() != &expectation.name {
            return contract_error(
                op,
                format!(
                    "attesa {} in posizione {position}, trovata {}",
                    expectation.name,
                    field.name()
                ),
            );
        }
        let expected = expected_type(op, &expectation.data_type)?;
        if !type_matches(field.data_type(), &expected) {
            return contract_error(
                op,
                format!(
                    "tipo errato per {}: atteso {}, trovato {}",
                    expectation.name,
                    expectation.data_type,
                    field.data_type()
                ),
            );
        }
        if expectation
            .nullable
            .is_some_and(|nullable| nullable != field.is_nullable())
        {
            return contract_error(
                op,
                format!("nullability errata per {}", expectation.name),
            );
        }
    }
    Ok(input.clone())
}

/// Replica `expected_type` del kernel quality.
fn expected_type(op: &str, value: &str) -> Result<DataType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "utf8" | "string" => Ok(DataType::Utf8),
        "int64" | "integer" => Ok(DataType::Int64),
        "float64" | "float" | "double" => Ok(DataType::Float64),
        "boolean" | "bool" => Ok(DataType::Boolean),
        "uint64" | "unsigned" => Ok(DataType::UInt64),
        "date32" => Ok(DataType::Date32),
        "timestamp_millis" => Ok(DataType::Timestamp(TimeUnit::Millisecond, None)),
        "decimal128" => Ok(DataType::Decimal128(38, 0)),
        "binary" => Ok(DataType::Binary),
        "dictionary_utf8" => Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        )),
        "list" => Ok(DataType::List(Arc::new(Field::new(
            "item",
            DataType::Null,
            true,
        )))),
        "struct" => Ok(DataType::Struct(plenora_core::arrow::schema::Fields::empty())),
        other => contract_error(op, format!("tipo non supportato {other}")),
    }
}

/// Replica `type_matches` del kernel quality (famiglie per tipi parametrici).
fn type_matches(actual: &DataType, expected: &DataType) -> bool {
    match expected {
        DataType::List(_) => matches!(actual, DataType::List(_)),
        DataType::Struct(_) => matches!(actual, DataType::Struct(_)),
        DataType::Timestamp(TimeUnit::Millisecond, None) => {
            matches!(actual, DataType::Timestamp(TimeUnit::Millisecond, _))
        }
        DataType::Decimal128(_, _) => matches!(actual, DataType::Decimal128(_, _)),
        _ => actual == expected,
    }
}

fn analyze_assert_not_null(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertNotNull = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in &config.columns {
        field_of(op, input, name)?;
    }
    Ok(input.clone())
}

fn analyze_assert_unique(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertUnique = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in &config.columns {
        require_scalar_string(op, input, name)?;
    }
    Ok(input.clone())
}

fn analyze_assert_range(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertRange = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    require_numeric(op, input, &config.column)?;
    Ok(input.clone())
}

fn analyze_assert_regex(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::AssertRegex = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    require_utf8(op, input, &config.column)?;
    regex::Regex::new(&config.pattern)
        .map_err(|error| PlenoraError::Contract(format!("{op}: regex non valida: {error}")))?;
    Ok(input.clone())
}

fn analyze_coalesce(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: quality::Coalesce = typed(op, config)?;
    let input = &inputs[0];
    check_output_name(op, &config.output_column)?;
    if config.columns.is_empty() {
        return contract_error(op, "coalesce richiede almeno una colonna");
    }
    let data_type = field_of(op, input, &config.columns[0])?.data_type().clone();
    for name in &config.columns[1..] {
        let field = field_of(op, input, name)?;
        if field.data_type() != &data_type {
            return contract_error(op, "coalesce richiede colonne con tipi Arrow identici");
        }
    }
    analyze_append(input, fields, &[(config.output_column, data_type, true)])
}

// ---------------------------------------------------------------------------
// governance.rs
// ---------------------------------------------------------------------------

fn analyze_assert_cardinality(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::AssertCardinality = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    // Con row_count Proven in input la cardinalita' e' verificabile a secco.
    if let Some(proven) = input
        .properties
        .row_count
        .as_ref()
        .and_then(|property| property.confidence.proven_value())
    {
        let rows = usize::try_from(*proven).unwrap_or(usize::MAX);
        let violated = config.exact_rows.map_or_else(
            || {
                config.min_rows.is_some_and(|min| min > rows)
                    || config.max_rows.is_some_and(|max| max < rows)
            },
            |exact| exact != rows,
        );
        if violated {
            return contract_error(
                op,
                format!("cardinalita' attestata incompatibile con row_count Proven({proven})"),
            );
        }
    }
    Ok(input.clone())
}

fn analyze_assert_metadata(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::AssertMetadata = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let metadata = input.schema.metadata();
    for (key, value) in &config.expected {
        if metadata.get(key) != Some(value) {
            return contract_error(op, format!("metadata {key:?} non conforme"));
        }
    }
    if !config.allow_extra && metadata.len() != config.expected.len() {
        return contract_error(op, "metadata extra non ammessi");
    }
    Ok(input.clone())
}

/// Chiavi di un'op binaria: esistenza nei due schemi e tipi delle coppie
/// identici (zip come i kernel governance, senza check di pari cardinalita').
fn check_foreign_keys(
    op: &str,
    left: &DataContract,
    right: &DataContract,
    left_keys: &[String],
    right_keys: &[String],
) -> Result<()> {
    for (left_key, right_key) in left_keys.iter().zip(right_keys) {
        let left_field = field_of(op, left, left_key)?;
        let right_field = field_of(op, right, right_key)?;
        if left_field.data_type() != right_field.data_type() {
            return contract_error(
                op,
                format!("chiavi {left_key}/{right_key} con tipi Arrow diversi"),
            );
        }
    }
    Ok(())
}

fn analyze_assert_foreign_key(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::ForeignKey = typed(op, config)?;
    let _ = fields;
    check_foreign_keys(op, &inputs[0], &inputs[1], &config.left_keys, &config.right_keys)?;
    // Right non contribuisce allo schema: output = left invariato.
    Ok(inputs[0].clone())
}

fn analyze_reconcile(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::Reconcile = typed(op, config)?;
    let _ = fields;
    check_foreign_keys(op, &inputs[0], &inputs[1], &config.left_keys, &config.right_keys)?;
    // Schema fisso: 5 righe di metriche, indipendente dagli input.
    let schema = Schema::new(vec![
        Field::new("metric", DataType::Utf8, false),
        Field::new("value", DataType::UInt64, false),
    ]);
    finish(
        schema,
        None,
        None,
        ContractProperties {
            sorted_by: None,
            row_count: Some(ContractProperty::new(
                PropertyConfidence::Proven(5),
                PropertyScope::Dataset,
            )),
        },
    )
}

fn analyze_validate_rules(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: governance::ValidateRules = typed(op, config)?;
    let input = &inputs[0];
    if config.rules.is_empty() {
        return contract_error(op, "rules vuoto");
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for rule in &config.rules {
        if rule.name.trim().is_empty() {
            return contract_error(op, "nome regola vuoto");
        }
        if !seen.insert(rule.name.as_str()) {
            return contract_error(op, format!("regola ripetuta: {}", rule.name));
        }
        let Some(column) = rule.column.as_deref() else {
            return contract_error(op, format!("regola {} senza column", rule.name));
        };
        let field = field_of(op, input, column)?;
        let needs_value = !matches!(
            rule.operator,
            governance::RuleOperator::Isnull | governance::RuleOperator::Notnull
        );
        if needs_value != rule.value.is_some() {
            return contract_error(
                op,
                format!(
                    "regola {}: value {} per l'operatore",
                    rule.name,
                    if needs_value { "obbligatorio" } else { "non ammesso" }
                ),
            );
        }
        let expected = rule.value.as_ref().map_or_else(String::new, json_text);
        match rule.operator {
            governance::RuleOperator::Isnull | governance::RuleOperator::Notnull => {}
            governance::RuleOperator::Eq | governance::RuleOperator::Ne => {
                if !governance::is_rule_comparable(field.data_type()) {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: tipo {:?} non confrontabile",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                if governance::is_rule_numeric(field.data_type())
                    && expected.parse::<f64>().is_err()
                {
                    return contract_error(
                        op,
                        format!("regola {}: confronto numerico con valore non numerico", rule.name),
                    );
                }
            }
            governance::RuleOperator::Gt
            | governance::RuleOperator::Ge
            | governance::RuleOperator::Lt
            | governance::RuleOperator::Le => {
                if !governance::is_rule_numeric(field.data_type()) {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: confronto ordinato richiede colonna numerica (tipo {:?})",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                if expected.parse::<f64>().is_err() {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: confronto ordinato richiede un valore numerico",
                            rule.name
                        ),
                    );
                }
            }
            governance::RuleOperator::Range => {
                if !governance::is_rule_numeric(field.data_type()) {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: range richiede colonna numerica (tipo {:?})",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                let Some((low, high)) = expected.split_once(',') else {
                    return contract_error(op, format!("regola {}: range richiede min,max", rule.name));
                };
                if low.trim().parse::<f64>().is_err() || high.trim().parse::<f64>().is_err() {
                    return contract_error(
                        op,
                        format!("regola {}: estremi range non numerici", rule.name),
                    );
                }
            }
            governance::RuleOperator::Regex => {
                if field.data_type() != &DataType::Utf8 {
                    return contract_error(
                        op,
                        format!(
                            "regola {}: regex richiede colonna Utf8 (tipo {:?})",
                            rule.name,
                            field.data_type()
                        ),
                    );
                }
                if expected.len() > Limits::default().max_regex_bytes {
                    return contract_error(op, format!("regola {}: pattern oltre max_regex_bytes", rule.name));
                }
                regex::Regex::new(&expected).map_err(|error| {
                    PlenoraError::Contract(format!(
                        "{op}: regola {}: regex non valida: {error}",
                        rule.name
                    ))
                })?;
            }
        }
    }
    match config.output_mode {
        governance::ValidateOutputMode::Annotate => analyze_append(
            input,
            fields,
            &[
                ("_valid".to_owned(), DataType::Boolean, false),
                ("_errors".to_owned(), DataType::Utf8, false),
                ("_warnings".to_owned(), DataType::Utf8, false),
            ],
        ),
        governance::ValidateOutputMode::Summary => {
            // Dataset nuovo: una riga per regola, nessuna colonna d'input.
            for name in ["name", "errors", "warnings"] {
                fields.derive(name);
            }
            let schema = Schema::new(vec![
                Field::new("name", DataType::Utf8, false),
                Field::new("errors", DataType::Int64, false),
                Field::new("warnings", DataType::Int64, false),
            ]);
            finish(schema, None, None, ContractProperties::default())
        }
    }
}

// ---------------------------------------------------------------------------
// reshape.rs
// ---------------------------------------------------------------------------

/// Replica `collision_free` del kernel melt: nome libero nello schema di
/// input, altrimenti `{name}_1..{name}_99`.
fn collision_free(op: &str, input: &DataContract, name: &str) -> Result<String> {
    check_output_name(op, name)?;
    if input.schema.index_of(name).is_err() {
        return Ok(name.to_owned());
    }
    (1..100)
        .map(|index| format!("{name}_{index}"))
        .find(|candidate| input.schema.index_of(candidate).is_err())
        .ok_or_else(|| {
            PlenoraError::Contract(format!("{op}: impossibile evitare collisione {name}"))
        })
}

fn analyze_melt(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Melt = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let id_indices: Vec<usize> = config
        .id_columns
        .iter()
        .map(|name| {
            input
                .schema
                .index_of(name)
                .map_err(|_| PlenoraError::Contract(format!("{op}: colonna non trovata: {name}")))
        })
        .collect::<Result<_>>()?;
    let value_indices: Vec<usize> = if config.value_columns.is_empty() {
        (0..input.schema.fields().len())
            .filter(|index| !id_indices.contains(index))
            .collect()
    } else {
        config
            .value_columns
            .iter()
            .map(|name| {
                input.schema.index_of(name).map_err(|_| {
                    PlenoraError::Contract(format!("{op}: colonna non trovata: {name}"))
                })
            })
            .collect::<Result<_>>()?
    };
    if value_indices.is_empty() {
        return contract_error(op, "melt senza value_columns");
    }
    let source_fields = clone_fields(input);
    let value_type = source_fields[value_indices[0]].data_type().clone();
    let homogeneous = value_indices
        .iter()
        .all(|index| source_fields[*index].data_type() == &value_type);
    let value_data_type = if homogeneous {
        value_type
    } else if matches!(config.type_policy, reshape::HeterogeneousTypePolicy::String) {
        DataType::Utf8
    } else {
        return contract_error(
            op,
            "value_columns eterogenee; impostare type_policy='string' per la conversione esplicita",
        );
    };
    let var_name = collision_free(op, input, &config.var_name)?;
    let value_name = collision_free(op, input, &config.value_name)?;
    let mut fields_out: Vec<Field> = id_indices
        .iter()
        .map(|index| source_fields[*index].clone())
        .collect();
    fields_out.push(Field::new(&var_name, DataType::Utf8, false));
    fields_out.push(Field::new(&value_name, value_data_type, true));
    // Schema::new nel kernel: metadata non preservati.
    let schema = Schema::new(fields_out);
    // La geometria sopravvive solo come colonna id (valori passthrough).
    let preserved = input
        .geometries
        .first()
        .filter(|geometry| config.id_columns.contains(&geometry.name))
        .map(|geometry| geometry.name.as_str());
    let geometry = propagate_geometry(input, &schema, preserved);
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    let value_count = u64::try_from(value_indices.len()).unwrap_or(u64::MAX);
    finish(
        schema,
        geometry,
        active,
        ContractProperties {
            sorted_by: None,
            row_count: map_row_count(input, |rows| rows.saturating_mul(value_count)),
        },
    )
}

fn analyze_pivot(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    // Valida la config e le colonne referenziate, poi fallisce: i nomi delle
    // colonne pivot derivano dai valori presenti nei dati (anche con mapping,
    // che filtra ma non garantisce la presenza).
    let config: reshape::Pivot = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    for name in config
        .index_col
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        field_of(op, input, name)?;
    }
    field_of(op, input, &config.column)?;
    field_of(op, input, &config.value_col)?;
    unsupported(
        op,
        "le colonne di output dipendono dai valori distinti della pivot_col: schema non inferibile a secco",
    )
}

fn analyze_transpose(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Transpose = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    if let Some(id_column) = &config.id_column {
        field_of(op, input, id_column)?;
    }
    unsupported(
        op,
        "il numero di colonne di output dipende dal numero di righe dell'input: schema non inferibile a secco",
    )
}

fn analyze_explode(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Explode = typed(op, config)?;
    let input = &inputs[0];
    let field = field_of(op, input, &config.column)?;
    let DataType::List(child) = field.data_type() else {
        return contract_error(op, "explode richiede una colonna List");
    };
    let element_type = child.data_type().clone();
    let output_name = config.output_column.unwrap_or_else(|| config.column.clone());
    check_output_name(op, &output_name)?;
    let mut output = analyze_append(input, fields, &[(output_name, element_type, true)])?;
    // Le righe cambiano (una per elemento); l'espansione puo' rompere
    // l'ordinamento se la colonna esplosa era una chiave: conservativo.
    output.properties = ContractProperties::default();
    Ok(output)
}

fn analyze_unnest(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::Unnest = typed(op, config)?;
    let input = &inputs[0];
    let _ = fields;
    let index = input
        .schema
        .index_of(&config.column)
        .map_err(|_| PlenoraError::Contract(format!("{op}: colonna non trovata: {}", config.column)))?;
    let field = input.schema.field(index);
    let DataType::Struct(children) = field.data_type() else {
        return contract_error(op, "unnest richiede una colonna Struct");
    };
    let projected = input
        .schema
        .fields()
        .len()
        .saturating_sub(usize::from(config.drop_source))
        .saturating_add(children.len());
    if projected > Limits::default().max_columns {
        return contract_error(op, "unnest supera max_columns");
    }
    let mut fields_out: Vec<Field> = Vec::with_capacity(projected);
    let mut names: HashSet<String> = HashSet::new();
    for (position, field) in clone_fields(input).into_iter().enumerate() {
        if position == index && config.drop_source {
            continue;
        }
        names.insert(field.name().clone());
        fields_out.push(field);
    }
    for child in children {
        let name = format!("{}{}", config.prefix, child.name());
        check_output_name(op, &name)?;
        if !names.insert(name.clone()) {
            return contract_error(op, format!("unnest: collisione colonna {name}"));
        }
        fields_out.push(child.as_ref().clone().with_name(name).with_nullable(true));
    }
    let schema = Schema::new_with_metadata(fields_out, input.schema.metadata().clone());
    let geometry = propagate_geometry(input, &schema, input.geometries.first().map(|g| g.name.as_str()));
    let active = input
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(
        schema,
        geometry,
        active,
        ContractProperties {
            // Con drop_source la colonna struct (possibile chiave) e' rimossa.
            sorted_by: if config.drop_source {
                None
            } else {
                input.properties.sorted_by.clone()
            },
            row_count: input.properties.row_count.clone(),
        },
    )
}

fn analyze_table_diff(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: reshape::TableDiff = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    if config.left_keys.is_empty() || config.left_keys.len() != config.right_keys.len() {
        return contract_error(op, "chiavi table_diff non valide");
    }
    check_key_pairs(op, left, right, &config.left_keys, &config.right_keys)?;
    let compare: Vec<String> = if config.compare_columns.is_empty() {
        // Default: colonne di left non chiave presenti anche in right.
        left.schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .filter(|name| {
                !config.left_keys.contains(name) && right.schema.index_of(name).is_ok()
            })
            .collect()
    } else {
        config.compare_columns.clone()
    };
    let mut fields_out: Vec<Field> = Vec::new();
    for name in &config.left_keys {
        let field = field_of(op, left, name)?;
        fields_out.push(Field::new(name, field.data_type().clone(), true));
    }
    for name in &compare {
        field_of(op, left, name)?;
        let right_field = field_of(op, right, name)?;
        if field_of(op, left, name)?.data_type() != right_field.data_type() {
            return contract_error(
                op,
                format!("colonna {name} con tipi Arrow diversi tra left e right"),
            );
        }
        fields_out.push(Field::new(name, right_field.data_type().clone(), true));
    }
    fields_out.push(Field::new("_diff_status", DataType::Utf8, false));
    fields_out.push(Field::new("_diff_columns", DataType::Utf8, true));
    fields_out.push(Field::new("_diff_old_values", DataType::Utf8, true));
    let schema = Schema::new(fields_out);
    let emitted = |name: &str| config.left_keys.contains(&name.to_owned()) || compare.iter().any(|c| c == name);
    let left_geometry = left
        .geometries
        .first()
        .filter(|geometry| emitted(&geometry.name))
        .and_then(|geometry| propagate_geometry(left, &schema, Some(&geometry.name)));
    let right_geometry = if left_geometry.is_none() {
        right
            .geometries
            .first()
            .filter(|geometry| emitted(&geometry.name))
            .and_then(|geometry| propagate_geometry(right, &schema, Some(&geometry.name)))
    } else {
        None
    };
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    finish(schema, geometry, None, ContractProperties::default())
}

// ---------------------------------------------------------------------------
// joins.rs / setops.rs
// ---------------------------------------------------------------------------

/// Chiavi di join con pari cardinalita' obbligatoria e tipi identici.
fn check_key_pairs(
    op: &str,
    left: &DataContract,
    right: &DataContract,
    left_keys: &[String],
    right_keys: &[String],
) -> Result<()> {
    if left_keys.is_empty() || left_keys.len() != right_keys.len() {
        return contract_error(op, "chiavi non valide (vuote o cardinalita' diversa)");
    }
    check_foreign_keys(op, left, right, left_keys, right_keys)
}

/// Replica `combine_horizontal`: tutte le colonne left (con naming per
/// variante), poi le colonne right non omesse; nullability forzata a true;
/// collisioni -> errore. Restituisce (campi, nome left, nome right) della
/// colonna geometrica propagata per ciascun ramo.
#[allow(clippy::too_many_arguments)]
fn combine_horizontal_fields(
    op: &str,
    left: &DataContract,
    right: &DataContract,
    omitted_right: &HashSet<usize>,
    naming: HorizontalNaming<'_>,
) -> Result<(Vec<Field>, Option<String>, Option<String>)> {
    let right_names: HashSet<&str> = right
        .schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(index, _)| !omitted_right.contains(index))
        .map(|(_, field)| field.name().as_str())
        .collect();
    let left_key_indices: HashSet<usize> = match naming {
        HorizontalNaming::ManipolaJoin(indices) => indices.iter().copied().collect(),
        _ => HashSet::new(),
    };
    let mut fields_out: Vec<Field> = Vec::new();
    let mut left_geometry_name = None;
    for (index, field) in clone_fields(left).into_iter().enumerate() {
        let original = field.name().clone();
        let name = match naming {
            HorizontalNaming::ManipolaJoin(_) if !left_key_indices.contains(&index) => {
                format!("{original}_L")
            }
            HorizontalNaming::PandasCross if right_names.contains(original.as_str()) => {
                format!("{original}_x")
            }
            _ => original.clone(),
        };
        check_output_name(op, &name)?;
        if left.geometries.first().is_some_and(|g| g.name == original) {
            left_geometry_name = Some(name.clone());
        }
        fields_out.push(field.with_name(name).with_nullable(true));
    }
    let mut names: HashSet<String> = fields_out
        .iter()
        .map(|field| field.name().clone())
        .collect();
    if names.len() != fields_out.len() {
        return contract_error(op, "collisione nomi nelle colonne sinistre del join");
    }
    let left_source_names: HashSet<&str> = left
        .schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    let mut right_geometry_name = None;
    for (index, field) in clone_fields(right).into_iter().enumerate() {
        if omitted_right.contains(&index) {
            continue;
        }
        let original = field.name().clone();
        let name = match naming {
            HorizontalNaming::ManipolaJoin(_) => format!("{original}_R"),
            HorizontalNaming::PandasCross if left_source_names.contains(original.as_str()) => {
                format!("{original}_y")
            }
            HorizontalNaming::AsOf if left_source_names.contains(original.as_str()) => {
                format!("{original}_R")
            }
            _ => original.clone(),
        };
        check_output_name(op, &name)?;
        if !names.insert(name.clone()) {
            return contract_error(op, format!("collisione join: {name}"));
        }
        if right.geometries.first().is_some_and(|g| g.name == original) {
            right_geometry_name = Some(name.clone());
        }
        fields_out.push(field.with_name(name).with_nullable(true));
    }
    if fields_out.len() > Limits::default().max_columns {
        return contract_error(op, "join supera max_columns");
    }
    Ok((fields_out, left_geometry_name, right_geometry_name))
}

#[derive(Clone, Copy)]
enum HorizontalNaming<'a> {
    ManipolaJoin(&'a [usize]),
    PandasCross,
    AsOf,
}

fn analyze_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::Join = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    check_key_pairs(op, left, right, &config.left_keys, &config.right_keys)?;
    let left_indices: Vec<usize> = config
        .left_keys
        .iter()
        .map(|name| left.schema.index_of(name).expect("chiave verificata"))
        .collect();
    let right_indices: HashSet<usize> = config
        .right_keys
        .iter()
        .map(|name| right.schema.index_of(name).expect("chiave verificata"))
        .collect();
    let (fields_out, left_geometry, right_geometry) = combine_horizontal_fields(
        op,
        left,
        right,
        &right_indices,
        HorizontalNaming::ManipolaJoin(&left_indices),
    )?;
    let schema = Schema::new(fields_out);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let active = left
        .active_geometry
        .filter(|id| left_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        .or_else(|| {
            right
                .active_geometry
                .filter(|id| right_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        });
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    finish(schema, geometry, active, ContractProperties::default())
}

/// `table.fuzzy_join` (estensione v1.3): replica le validazioni statiche del
/// kernel (`fuzzy::validate_config`, chiavi Utf8 esistenti) e inferisce lo
/// schema del join Manipola con la chiave destra INCLUSA (suffisso `_R`,
/// nel fuzzy le due chiavi differiscono) piu' la colonna score Float64 in
/// coda (nullable solo con `how = left`). Proprieta' declassate: il numero di
/// righe dipende dai dati e l'ordine non e' quello di nessun input.
fn analyze_fuzzy_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: fuzzy::FuzzyJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    fuzzy::validate_config(&config)
        .map_err(|error| PlenoraError::Contract(format!("{op}: {error}")))?;
    require_utf8(op, left, &config.left_key)?;
    require_utf8(op, right, &config.right_key)?;
    let left_index = left.schema.index_of(&config.left_key).expect("chiave verificata");
    let (mut fields_out, left_geometry, right_geometry) = combine_horizontal_fields(
        op,
        left,
        right,
        &HashSet::new(),
        HorizontalNaming::ManipolaJoin(&[left_index]),
    )?;
    let score_name = config.score_name();
    check_output_name(op, score_name)?;
    if fields_out.iter().any(|field| field.name() == score_name) {
        return contract_error(op, format!("collisione fuzzy_join: {score_name}"));
    }
    fields_out.push(Field::new(
        score_name,
        DataType::Float64,
        config.how == fuzzy::FuzzyHow::Left,
    ));
    if fields_out.len() > Limits::default().max_columns {
        return contract_error(op, "fuzzy_join supera max_columns");
    }
    let schema = Schema::new(fields_out);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    finish(schema, geometry, None, ContractProperties::default())
}

fn analyze_cross_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let _config: joins::CrossJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    let (fields_out, left_geometry, right_geometry) = combine_horizontal_fields(
        op,
        left,
        right,
        &HashSet::new(),
        HorizontalNaming::PandasCross,
    )?;
    let schema = Schema::new(fields_out);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    // Righe = prodotto esatto delle cardinalita' (se note e stesso scope).
    let row_count = product_row_count(left, right);
    finish(
        schema,
        geometry,
        None,
        ContractProperties {
            sorted_by: None,
            row_count,
        },
    )
}

fn product_row_count(
    left: &DataContract,
    right: &DataContract,
) -> Option<ContractProperty<u64>> {
    let left_property = left.properties.row_count.as_ref()?;
    let right_property = right.properties.row_count.as_ref()?;
    if left_property.scope != right_property.scope {
        return None;
    }
    let combine = |a: &u64, b: &u64| a.checked_mul(*b);
    let confidence = match (&left_property.confidence, &right_property.confidence) {
        (PropertyConfidence::Proven(a), PropertyConfidence::Proven(b)) => {
            PropertyConfidence::Proven(combine(a, b)?)
        }
        (PropertyConfidence::Estimated(a), PropertyConfidence::Estimated(b)) => {
            PropertyConfidence::Estimated(combine(a, b)?)
        }
        _ => return None,
    };
    Some(ContractProperty::new(confidence, left_property.scope))
}

fn analyze_membership_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::MembershipJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    check_key_pairs(op, left, right, &config.left_keys, &config.right_keys)?;
    // Output = left via select_rows: schema invariato, ordine preservato.
    let mut output = left.clone();
    output.properties = sorted_only(left);
    Ok(output)
}

fn analyze_asof_join(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::AsOfJoin = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    if config.left_by.len() != config.right_by.len() {
        return contract_error(op, "left_by/right_by di cardinalita' diversa");
    }
    if config
        .tolerance
        .is_some_and(|tolerance| !tolerance.is_finite() || tolerance < 0.0)
    {
        return contract_error(op, "tolerance deve essere finita e >= 0");
    }
    let left_on = field_of(op, left, &config.left_on)?;
    let right_on = field_of(op, right, &config.right_on)?;
    if left_on.data_type() != right_on.data_type()
        || !matches!(left_on.data_type(), DataType::Int64 | DataType::Float64)
    {
        return contract_error(op, "left_on/right_on devono avere tipo identico Int64 o Float64");
    }
    check_foreign_keys(op, left, right, &config.left_by, &config.right_by)?;
    let omitted: HashSet<usize> = config
        .right_by
        .iter()
        .chain(std::iter::once(&config.right_on))
        .map(|name| right.schema.index_of(name).expect("chiave verificata"))
        .collect();
    let (fields_out, left_geometry, right_geometry) =
        combine_horizontal_fields(op, left, right, &omitted, HorizontalNaming::AsOf)?;
    let schema = Schema::new(fields_out);
    let left_geometry = propagate_geometry(left, &schema, left_geometry.as_deref());
    let right_geometry = propagate_geometry(right, &schema, right_geometry.as_deref());
    let active = left
        .active_geometry
        .filter(|id| left_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        .or_else(|| {
            right
                .active_geometry
                .filter(|id| right_geometry.as_ref().is_some_and(|g| &g.field_id == id))
        });
    let geometry = merge_geometry(op, left_geometry, right_geometry)?;
    // Una riga di output per riga left, nell'ordine di left.
    finish(schema, geometry, active, left.properties.clone())
}

fn analyze_concat(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let _config: joins::Concat = typed(op, config)?;
    let _ = fields;
    let first = &inputs[0];
    for other in &inputs[1..] {
        check_same_schema(op, first, other)?;
    }
    let fields_out: Vec<Field> = first
        .schema
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let nullable = inputs
                .iter()
                .any(|input| input.schema.field(index).is_nullable());
            field.as_ref().clone().with_nullable(nullable)
        })
        .collect();
    let schema = Schema::new_with_metadata(fields_out, first.schema.metadata().clone());
    // Geometria del primo input (gli schemi sono identici per nome/tipo).
    let geometry = propagate_geometry(first, &schema, first.geometries.first().map(|g| g.name.as_str()));
    let row_count = sum_row_count(inputs);
    finish(
        schema,
        geometry,
        first.active_geometry,
        ContractProperties {
            sorted_by: None,
            row_count,
        },
    )
}

fn sum_row_count(inputs: &[DataContract]) -> Option<ContractProperty<u64>> {
    let first = inputs.first()?.properties.row_count.as_ref()?;
    if inputs.iter().any(|input| {
        input
            .properties
            .row_count
            .as_ref()
            .is_none_or(|property| property.scope != first.scope)
    }) {
        return None;
    }
    let confidence = match &first.confidence {
        PropertyConfidence::Proven(_) => {
            let mut total = 0_u64;
            for input in inputs {
                let value = input.properties.row_count.as_ref()?.confidence.proven_value()?;
                total = total.checked_add(*value)?;
            }
            PropertyConfidence::Proven(total)
        }
        PropertyConfidence::Estimated(_) => {
            let mut total = 0_u64;
            for input in inputs {
                let value = input.properties.row_count.as_ref()?.confidence.value().copied()?;
                if !matches!(
                    input.properties.row_count.as_ref()?.confidence,
                    PropertyConfidence::Estimated(_)
                ) {
                    return None;
                }
                total = total.checked_add(value)?;
            }
            PropertyConfidence::Estimated(total)
        }
        _ => return None,
    };
    Some(ContractProperty::new(confidence, first.scope))
}

/// Replica `validate_schema`/`concat_compatible` del kernel: stesso numero di
/// colonne, nomi e `DataType` identici campo per campo (nullability ignorata).
fn check_same_schema(op: &str, left: &DataContract, right: &DataContract) -> Result<()> {
    if left.schema.fields().len() != right.schema.fields().len() {
        return contract_error(op, "schemi con numero di colonne diverso");
    }
    for (left_field, right_field) in left
        .schema
        .fields()
        .iter()
        .zip(right.schema.fields().iter())
    {
        if left_field.name() != right_field.name()
            || left_field.data_type() != right_field.data_type()
        {
            return contract_error(
                op,
                format!(
                    "schemi incompatibili: {} ({:?}) vs {} ({:?})",
                    left_field.name(),
                    left_field.data_type(),
                    right_field.name(),
                    right_field.data_type()
                ),
            );
        }
    }
    Ok(())
}

fn analyze_concat_by_name(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
) -> Result<DataContract> {
    let config: joins::ConcatByName = typed(op, config)?;
    let _ = fields;
    let first = &inputs[0];
    // Schema unione: stesse regole del kernel (ordine di prima apparizione,
    // tipi identici per nome, nullable se assente in almeno un input).
    let fields_out: Vec<Field> = if config.strict {
        for other in &inputs[1..] {
            check_same_schema(op, first, other)?;
        }
        first
            .schema
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let nullable = inputs
                    .iter()
                    .any(|input| input.schema.field(index).is_nullable());
                field.as_ref().clone().with_nullable(nullable)
            })
            .collect()
    } else {
        let mut union: Vec<Field> = Vec::new();
        for input in inputs {
            for field in input.schema.fields() {
                if let Some(existing) = union.iter_mut().find(|f| f.name() == field.name()) {
                    if existing.data_type() != field.data_type() {
                        return contract_error(
                            op,
                            format!(
                                "tipi incompatibili per la colonna {} ({:?} vs {:?})",
                                field.name(),
                                existing.data_type(),
                                field.data_type()
                            ),
                        );
                    }
                    if field.is_nullable() && !existing.is_nullable() {
                        *existing = existing.clone().with_nullable(true);
                    }
                } else {
                    union.push(field.as_ref().clone());
                }
            }
        }
        for field in &mut union {
            if !field.is_nullable()
                && inputs
                    .iter()
                    .any(|input| input.schema.field_with_name(field.name()).is_err())
            {
                *field = field.clone().with_nullable(true);
            }
        }
        union
    };
    let schema = Schema::new_with_metadata(fields_out, first.schema.metadata().clone());
    // Geometria: propagata solo se TUTTI gli input hanno la colonna con lo
    // stesso tipo (altrimenti le righe degli input senza la colonna
    // sarebbero null materializzati, non passthrough D16).
    let geometry = first
        .geometries
        .first()
        .filter(|geometry| {
            inputs.iter().all(|input| {
                input
                    .schema
                    .field_with_name(&geometry.name)
                    .is_ok_and(|field| {
                        Some(field.data_type())
                            == first
                                .schema
                                .field_with_name(&geometry.name)
                                .ok()
                                .map(|source| source.data_type())
                    })
            })
        })
        .and_then(|geometry| propagate_geometry(first, &schema, Some(&geometry.name)));
    let active = first
        .active_geometry
        .filter(|id| geometry.as_ref().is_some_and(|g| &g.field_id == id));
    finish(
        schema,
        geometry,
        active,
        ContractProperties {
            sorted_by: None,
            row_count: sum_row_count(inputs),
        },
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetOp {
    UnionDistinct,
    Intersect,
    Except,
}

fn analyze_set_operation(
    op: &str,
    inputs: &[DataContract],
    config: &Value,
    fields: &mut FieldAllocator,
    set_op: SetOp,
) -> Result<DataContract> {
    let _config: setops::SetOperation = typed(op, config)?;
    let (left, right) = (&inputs[0], &inputs[1]);
    let _ = fields;
    check_same_schema(op, left, right)?;
    // L'encoder delle chiavi di riga scatta prima dei dati: fail-closed.
    for field in left.schema.fields() {
        if !is_scalar_string(field.data_type()) {
            return contract_error(
                op,
                format!("tipo {:?} non supportato dalle set operation", field.data_type()),
            );
        }
    }
    if set_op == SetOp::UnionDistinct {
        let fields_out: Vec<Field> = left
            .schema
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                field
                    .as_ref()
                    .clone()
                    .with_nullable(field.is_nullable() || right.schema.field(index).is_nullable())
            })
            .collect();
        let schema = Schema::new_with_metadata(fields_out, left.schema.metadata().clone());
        let geometry = propagate_geometry(left, &schema, left.geometries.first().map(|g| g.name.as_str()));
        finish(schema, geometry, left.active_geometry, ContractProperties::default())
    } else {
        // intersect/except: output = left via select_rows, ordine canonico.
        let mut output = left.clone();
        output.properties = ContractProperties::default();
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use plenora_core::catalog::{Family, CATALOG};
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use serde_json::json;

    use super::*;

    fn projected_crs() -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            "EPSG:32632".to_owned(),
            serde_json::json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn base_fields() -> Vec<Field> {
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("value", DataType::Float64, true),
            Field::new("flag", DataType::Boolean, true),
            Field::new("d", DataType::Date32, true),
            Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new("dc", DataType::Decimal128(10, 2), true),
            Field::new("u", DataType::UInt64, true),
            Field::new("lst", DataType::List(Arc::new(Field::new("item", DataType::Int64, true))), true),
            Field::new(
                "st",
                DataType::Struct(vec![
                    Field::new("a", DataType::Int64, true),
                    Field::new("b", DataType::Utf8, true),
                ]
                .into()),
                true,
            ),
            Field::new("geom", DataType::Binary, true),
        ]
    }

    fn geometry(field_id: u32, name: &str, nullable: bool) -> GeometryColumnContract {
        GeometryColumnContract {
            field_id: FieldId(field_id),
            name: name.to_owned(),
            crs: projected_crs(),
            dimensions: plenora_core::contract::GeometryDimensions::Xy,
            nullable,
        }
    }

    /// Contratto con geometria attiva `geom` (FieldId(7)) e nessuna proprieta'.
    fn geo_contract() -> DataContract {
        DataContract::new(
            Arc::new(Schema::new(base_fields())),
            vec![geometry(7, "geom", true)],
            Some(FieldId(7)),
            ContractProperties::default(),
        )
        .unwrap()
    }

    /// Come `geo_contract`, con `sorted_by = Proven([FieldId(0)], Stream)` e
    /// `row_count = Proven(100, Dataset)`.
    fn proven_contract() -> DataContract {
        DataContract::new(
            Arc::new(Schema::new(base_fields())),
            vec![geometry(7, "geom", true)],
            Some(FieldId(7)),
            ContractProperties {
                sorted_by: Some(ContractProperty::new(
                    PropertyConfidence::Proven(vec![FieldId(0)]),
                    PropertyScope::Stream,
                )),
                row_count: Some(ContractProperty::new(
                    PropertyConfidence::Proven(100),
                    PropertyScope::Dataset,
                )),
            },
        )
        .unwrap()
    }

    fn tabular_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(base_fields())))
    }

    /// Contratto right per le op binarie: chiave `rid` Int64, colonne
    /// condivise `name`/`value` e colonna propria `rname`.
    fn right_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("rid", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("value", DataType::Float64, true),
            Field::new("rname", DataType::Utf8, true),
        ])))
    }

    /// Coppia di contratti semplici per concat/setops (niente List/Struct).
    fn simple_pair() -> (DataContract, DataContract) {
        let make = |nullable: bool, rows: u64| {
            DataContract::new(
                Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, nullable),
                ])),
                Vec::new(),
                None,
                ContractProperties {
                    sorted_by: None,
                    row_count: Some(ContractProperty::new(
                        PropertyConfidence::Proven(rows),
                        PropertyScope::Dataset,
                    )),
                },
            )
            .unwrap()
        };
        (make(false, 100), make(true, 50))
    }

    // Config posseduta per ergonomia dei test (json! inline).
    #[allow(clippy::needless_pass_by_value)]
    fn ok(op: &str, inputs: &[DataContract], config: Value) -> DataContract {
        analyze_table_contract(op, inputs, &config, &mut FieldAllocator::default())
            .unwrap_or_else(|error| panic!("{op} con config valida fallisce: {error}"))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn err(op: &str, inputs: &[DataContract], config: Value) -> PlenoraError {
        analyze_table_contract(op, inputs, &config, &mut FieldAllocator::default())
            .expect_err(&format!("{op} con config invalida deve fallire"))
    }

    fn assert_field(contract: &DataContract, name: &str, data_type: &DataType, nullable: bool) {
        let field = contract
            .schema
            .field_with_name(name)
            .unwrap_or_else(|_| panic!("colonna {name} assente dallo schema di output"));
        assert_eq!(
            field.data_type(),
            data_type,
            "tipo della colonna {name}"
        );
        assert_eq!(field.is_nullable(), nullable, "nullability di {name}");
    }

    fn proven_sorted_keys(contract: &DataContract) -> &Vec<FieldId> {
        let property = contract
            .properties
            .sorted_by
            .as_ref()
            .expect("sorted_by assente");
        assert!(property.is_proven(), "sorted_by non Proven");
        assert_eq!(property.scope, PropertyScope::Stream);
        property.confidence.proven_value().unwrap()
    }

    fn proven_rows(contract: &DataContract) -> u64 {
        let property = contract
            .properties
            .row_count
            .as_ref()
            .expect("row_count assente");
        *property.confidence.proven_value().expect("row_count non Proven")
    }

    // -- Completezza del dispatch -------------------------------------------

    #[test]
    fn every_table_op_has_an_analysis_arm() {
        let table_ops: Vec<_> = CATALOG
            .iter()
            .filter(|op| op.family == Family::Table)
            .collect();
        assert_eq!(table_ops.len(), 71);
        for descriptor in table_ops {
            let inputs = vec![tabular_contract(), right_contract()];
            let result =
                analyze_table_contract(descriptor.id, &inputs, &json!({}), &mut FieldAllocator::default());
            if let Err(PlenoraError::Unsupported(message)) = &result {
                assert!(
                    !message.contains("analyze_contract non disponibile"),
                    "{} senza braccio di analisi",
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn unknown_and_geo_ops_are_unsupported() {
        let inputs = vec![tabular_contract()];
        assert!(matches!(
            analyze_table_contract("table.nonexistent", &inputs, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Unsupported(_))
        ));
        assert!(matches!(
            analyze_table_contract("geo.buffer", &inputs, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Unsupported(_))
        ));
    }

    #[test]
    fn arity_is_enforced() {
        let one = vec![tabular_contract()];
        let three = [tabular_contract(), tabular_contract(), tabular_contract()];
        assert!(matches!(
            analyze_table_contract("table.join", &one, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Contract(_))
        ));
        assert!(matches!(
            analyze_table_contract("table.filter", &three[..2], &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Contract(_))
        ));
        assert!(matches!(
            analyze_table_contract("table.concat", &one, &json!({}), &mut FieldAllocator::default()),
            Err(PlenoraError::Contract(_))
        ));
        // concat N-aria: 3 input ammessi.
        let (a, b) = simple_pair();
        let (_, c) = simple_pair();
        assert!(analyze_table_contract("table.concat", &[a, b, c], &json!({}), &mut FieldAllocator::default()).is_ok());
    }

    // -- utility / strings / security / dates --------------------------------

    #[test]
    fn add_row_number_appends_int64_and_preserves_row_count() {
        let output = ok("table.add_row_number", &[proven_contract()], json!({}));
        assert_field(&output, "row_number", &DataType::Int64, false);
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(proven_rows(&output), 100);
        assert!(output.properties.sorted_by.is_some());
        assert!(matches!(
            err("table.add_row_number", &[tabular_contract()], json!({"order_column": "id"})),
            PlenoraError::Contract(_)
        ));
    }

    #[test]
    fn date_extract_appends_one_int64_per_part() {
        let output = ok(
            "table.date_extract",
            &[tabular_contract()],
            json!({"column": "name", "parts": ["year", "week"]}),
        );
        assert_field(&output, "name_year", &DataType::Int64, true);
        assert_field(&output, "name_week", &DataType::Int64, true);
        assert!(err("table.date_extract", &[tabular_contract()], json!({"column": "missing"})).to_string().contains("missing"));
    }

    #[test]
    fn uuid_generator_appends_non_nullable_utf8() {
        let output = ok("table.uuid_generator", &[tabular_contract()], json!({}));
        assert_field(&output, "uuid", &DataType::Utf8, false);
        assert!(err("table.uuid_generator", &[tabular_contract()], json!({"output_column": " "})).to_string().contains("nome"));
    }

    #[test]
    fn string_ops_validate_utf8_and_produce_expected_types() {
        let padded = ok("table.string_pad", &[tabular_contract()], json!({"column": "name"}));
        assert_field(&padded, "name", &DataType::Utf8, true);
        assert!(err("table.string_pad", &[tabular_contract()], json!({"column": "value"})).to_string().contains("Utf8"));
        assert!(err("table.string_pad", &[tabular_contract()], json!({"column": "name", "fill_char": "ab"})).to_string().contains("fill_char"));

        let length = ok("table.string_length", &[tabular_contract()], json!({"column": "name"}));
        assert_field(&length, "name_length", &DataType::Int64, true);
        assert!(err("table.string_length", &[tabular_contract()], json!({"column": "flag"})).to_string().contains("Utf8"));

        let extracted = ok(
            "table.string_extract",
            &[tabular_contract()],
            json!({"column": "name", "pattern": "(?P<year>\\d{4})-(?P<month>\\d{2})"}),
        );
        assert_field(&extracted, "year", &DataType::Utf8, true);
        assert_field(&extracted, "month", &DataType::Utf8, true);
        let plain = ok(
            "table.string_extract",
            &[tabular_contract()],
            json!({"column": "name", "pattern": "\\d+"}),
        );
        assert_field(&plain, "name_extracted", &DataType::Utf8, true);
        assert!(err("table.string_extract", &[tabular_contract()], json!({"column": "name", "pattern": "("})).to_string().contains("regex"));

        let normalized = ok("table.text_normalize", &[tabular_contract()], json!({"columns": ["name"]}));
        assert_field(&normalized, "name", &DataType::Utf8, true);
        assert!(err("table.text_normalize", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));
    }

    #[test]
    fn hash_and_mask_ops_append_utf8() {
        let md5 = ok("table.md5_hash", &[tabular_contract()], json!({"columns": ["name"]}));
        assert_field(&md5, "md5_hash", &DataType::Utf8, false);
        assert!(err("table.md5_hash", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));

        let sha = ok("table.sha256_hash", &[tabular_contract()], json!({"columns": ["name"]}));
        assert_field(&sha, "sha256_hash", &DataType::Utf8, false);
        assert!(err("table.sha256_hash", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));

        let masked = ok(
            "table.mask_data",
            &[tabular_contract()],
            json!({"maskings": [{"column": "name"}]}),
        );
        assert_field(&masked, "name_masked", &DataType::Utf8, true);
        let overwritten = ok(
            "table.mask_data",
            &[tabular_contract()],
            json!({"maskings": [{"column": "name"}], "overwrite": true}),
        );
        assert!(overwritten.schema.field_with_name("name_masked").is_err());
        assert_field(&overwritten, "name", &DataType::Utf8, true);
        assert!(err("table.mask_data", &[tabular_contract()], json!({"maskings": []})).to_string().contains("maskings"));
    }

    #[test]
    fn v1_1_extensions_analyze_contracts() {
        // select_columns: proiezione nell'ordine dato, geometria propagata se
        // selezionata inalterata.
        let output = ok(
            "table.select_columns",
            &[geo_contract()],
            json!({"columns": ["id", "geom"]}),
        );
        assert_eq!(output.schema.fields().len(), 2);
        assert_eq!(output.schema.field(0).name(), "id");
        assert_eq!(output.schema.field(1).name(), "geom");
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        // Geometria non selezionata: contratto tabellare.
        let output = ok(
            "table.select_columns",
            &[geo_contract()],
            json!({"columns": ["name", "id"]}),
        );
        assert!(output.geometries.is_empty());
        assert!(output.active_geometry.is_none());
        // Una colonna rimossa puo' essere chiave: sorted_by eliminato,
        // row_count invariato (1:1 sulle righe).
        let output = ok(
            "table.select_columns",
            &[proven_contract()],
            json!({"columns": ["id"]}),
        );
        assert!(output.properties.sorted_by.is_none());
        assert_eq!(proven_rows(&output), 100);
        assert!(err("table.select_columns", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));
        assert!(err("table.select_columns", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));
        assert!(err("table.select_columns", &[tabular_contract()], json!({"columns": ["id", "id"]})).to_string().contains("ripetuta"));

        // limit: schema invariato, ordine preservato, row_count non esatto.
        let output = ok("table.limit", &[proven_contract()], json!({"n": 10}));
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert!(output.properties.sorted_by.is_some());
        assert!(output.properties.row_count.is_none());
        assert!(err("table.limit", &[tabular_contract()], json!({})).to_string().contains("config"));

        // top_n: sorted_by Proven sulle chiavi, row_count = min(n, righe).
        let output = ok(
            "table.top_n",
            &[proven_contract()],
            json!({"columns": ["value"], "n": 10}),
        );
        assert_eq!(proven_sorted_keys(&output).len(), 1);
        assert_eq!(proven_rows(&output), 10);
        let output = ok(
            "table.top_n",
            &[proven_contract()],
            json!({"columns": ["value"], "n": 10_000}),
        );
        assert_eq!(proven_rows(&output), 100);
        assert!(err("table.top_n", &[tabular_contract()], json!({"columns": [], "n": 1})).to_string().contains("columns"));
        assert!(err("table.top_n", &[tabular_contract()], json!({"columns": ["missing"], "n": 1})).to_string().contains("missing"));

        // stable_fingerprint: append Utf8 non nullable; colonne validate.
        let output = ok(
            "table.stable_fingerprint",
            &[tabular_contract()],
            json!({"columns": ["id", "name"]}),
        );
        assert_field(&output, "fingerprint", &DataType::Utf8, false);
        assert!(err("table.stable_fingerprint", &[tabular_contract()], json!({"columns": ["lst"]})).to_string().contains("scalare"));
        assert!(err("table.stable_fingerprint", &[tabular_contract()], json!({"columns": ["id", "id"]})).to_string().contains("ripetuta"));
        // Default (tutte le colonne): lo schema di test contiene List/Struct,
        // non leggibili via profilo scalare -> fail-closed in validazione.
        assert!(err("table.stable_fingerprint", &[tabular_contract()], json!({})).to_string().contains("scalare"));
    }

    #[test]
    fn v1_2_extensions_analyze_contracts() {
        // align_schema: riordino/proiezione + colonna aggiunta di null (o
        // default non nullable); mismatch di tipo -> errore (mai cast).
        let output = ok(
            "table.align_schema",
            &[proven_contract()],
            json!({"columns": [
                {"name": "name", "type": "Utf8"},
                {"name": "id", "type": "Int64"},
                {"name": "note", "type": "Utf8"},
                {"name": "score", "type": "Float64", "default": 0}
            ]}),
        );
        let names: Vec<_> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["name", "id", "note", "score"]);
        assert_field(&output, "note", &DataType::Utf8, true);
        assert_field(&output, "score", &DataType::Float64, false);
        // Colonne rimosse/aggiunte: sorted_by eliminato, row_count invariato.
        assert!(output.properties.sorted_by.is_none());
        assert_eq!(proven_rows(&output), 100);
        // Permutazione pura con keep_extra: sorted_by preservato.
        let output = ok(
            "table.align_schema",
            &[proven_contract()],
            json!({"columns": [
                {"name": "name", "type": "Utf8"},
                {"name": "id", "type": "Int64"}
            ], "keep_extra": true}),
        );
        assert!(output.properties.sorted_by.is_some());
        // Geometria dichiarata col tipo giusto (Binary) e' propagata.
        let output = ok(
            "table.align_schema",
            &[geo_contract()],
            json!({"columns": [{"name": "geom", "type": "Binary"}, {"name": "id", "type": "Int64"}]}),
        );
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        // Errori: tipo diverso, default non convertibile, duplicati, vuoto.
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": [{"name": "id", "type": "Utf8"}]})).to_string().contains("cast implicito"));
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": [{"name": "x", "type": "Int64", "default": "abc"}]})).to_string().contains("default"));
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": [{"name": "id", "type": "Int64"}, {"name": "id", "type": "Int64"}]})).to_string().contains("ripetuta"));
        assert!(err("table.align_schema", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));

        // concat_by_name: unione per nome su schemi diversi.
        let (a, b) = simple_pair();
        let third = DataContract::new(
            Arc::new(Schema::new(vec![
                Field::new("name", DataType::Utf8, true),
                Field::new("extra", DataType::Int64, false),
            ])),
            Vec::new(),
            None,
            ContractProperties {
                sorted_by: None,
                row_count: Some(ContractProperty::new(
                    PropertyConfidence::Proven(25),
                    PropertyScope::Dataset,
                )),
            },
        )
        .unwrap();
        let output = ok("table.concat_by_name", &[a, b, third], json!({}));
        let names: Vec<_> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(names, ["id", "name", "extra"]);
        // id manca nel terzo input -> nullable; row_count = somma esatta.
        assert_field(&output, "id", &DataType::Int64, true);
        assert_eq!(proven_rows(&output), 175);
        // Tipi incompatibili per nome: errore.
        let incompatible = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
            "id",
            DataType::Utf8,
            true,
        )])));
        let (a, b) = simple_pair();
        assert!(err("table.concat_by_name", &[a, incompatible, b], json!({})).to_string().contains("incompatibili"));
        // Strict: schemi permutati -> errore.
        let permuted = DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("id", DataType::Int64, false),
        ])));
        let (a, _) = simple_pair();
        assert!(err("table.concat_by_name", &[a, permuted], json!({"strict": true})).to_string().contains("schemi"));

        // validate_rules annotate: tre colonne non nullable, row_count 1:1.
        let output = ok(
            "table.validate_rules",
            &[proven_contract()],
            json!({"rules": [
                {"name": "r1", "operator": "gt", "column": "value", "value": 0},
                {"name": "r2", "operator": "regex", "column": "name", "value": "^[a-z]+$", "severity": "warning"}
            ]}),
        );
        assert_field(&output, "_valid", &DataType::Boolean, false);
        assert_field(&output, "_errors", &DataType::Utf8, false);
        assert_field(&output, "_warnings", &DataType::Utf8, false);
        assert_eq!(proven_rows(&output), 100);
        // Summary: dataset nuovo (name, errors, warnings).
        let output = ok(
            "table.validate_rules",
            &[tabular_contract()],
            json!({"output_mode": "summary", "rules": [
                {"name": "r1", "operator": "isnull", "column": "name"}
            ]}),
        );
        assert_eq!(output.schema.fields().len(), 3);
        assert_field(&output, "name", &DataType::Utf8, false);
        assert_field(&output, "errors", &DataType::Int64, false);
        assert!(output.geometries.is_empty());
        // Errori di validazione: regex invalida, ordinato su Utf8, regex su
        // numerica, value mancante/non ammesso, regola duplicata.
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "regex", "column": "name", "value": "("}]})).to_string().contains("regex"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "gt", "column": "name", "value": 1}]})).to_string().contains("numerica"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "regex", "column": "id", "value": "1"}]})).to_string().contains("Utf8"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "eq", "column": "id"}]})).to_string().contains("value"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [{"name": "r", "operator": "isnull", "column": "id", "value": 1}]})).to_string().contains("value"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": [
            {"name": "r", "operator": "isnull", "column": "id"},
            {"name": "r", "operator": "notnull", "column": "id"}
        ]})).to_string().contains("ripetuta"));
        assert!(err("table.validate_rules", &[tabular_contract()], json!({"rules": []})).to_string().contains("rules"));

        // hmac_sha256: append Utf8; nullable solo con null_policy "null".
        let output = ok(
            "table.hmac_sha256",
            &[tabular_contract()],
            json!({"columns": ["id", "name"], "key_env": "PLENORA_HMAC"}),
        );
        assert_field(&output, "hmac", &DataType::Utf8, false);
        let output = ok(
            "table.hmac_sha256",
            &[tabular_contract()],
            json!({"columns": ["id"], "key_env": "PLENORA_HMAC", "null_policy": "null"}),
        );
        assert_field(&output, "hmac", &DataType::Utf8, true);
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": [], "key_env": "K"})).to_string().contains("columns"));
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": ["id"], "key_env": " "})).to_string().contains("key_env"));
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": ["lst"], "key_env": "K"})).to_string().contains("scalare"));
        assert!(err("table.hmac_sha256", &[tabular_contract()], json!({"columns": ["id", "id"], "key_env": "K"})).to_string().contains("ripetuta"));
    }

    #[test]
    fn v1_3_extensions_analyze_contracts() {
        // fuzzy_join: naming Manipola con chiave destra INCLUSA (_R) e score
        // Float64 in coda (non nullable in inner); proprieta' declassate.
        let inputs = [proven_contract(), right_contract()];
        let output = ok(
            "table.fuzzy_join",
            &inputs,
            json!({"left_key": "name", "right_key": "rname", "metric": "jaro_winkler", "threshold": 0.9, "blocking": "prefix"}),
        );
        let names: Vec<_> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(
            names.last(),
            Some(&"score"),
            "score in coda: {names:?}"
        );
        assert!(names.contains(&"rname_R"), "chiave destra inclusa: {names:?}");
        assert!(names.contains(&"name"), "chiave sinistra conserva il nome: {names:?}");
        assert_field(&output, "score", &DataType::Float64, false);
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());
        // how=left: score nullable; nome personalizzato.
        let output = ok(
            "table.fuzzy_join",
            &[tabular_contract(), right_contract()],
            json!({"left_key": "name", "right_key": "rname", "metric": "levenshtein", "threshold": 0.5, "blocking": "soundex", "how": "left", "score_column": "similarity"}),
        );
        assert_field(&output, "similarity", &DataType::Float64, true);
        // Soglia validata in analisi: 0 escluso, oltre 1 e NaN rifiutati.
        for bad in [json!(0.0), json!(-0.1), json!(1.5), json!(null)] {
            let mut config = json!({"left_key": "name", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "none"});
            config["threshold"] = bad;
            assert!(
                err("table.fuzzy_join", &[tabular_contract(), right_contract()], config)
                    .to_string()
                    .contains("fuzzy_join")
            );
        }
        // Chiavi mancanti o non Utf8, blocking_param fuori posto, collisione score.
        assert!(err("table.fuzzy_join", &[tabular_contract(), right_contract()], json!({"left_key": "missing", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "prefix"})).to_string().contains("missing"));
        assert!(err("table.fuzzy_join", &[tabular_contract(), right_contract()], json!({"left_key": "id", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "prefix"})).to_string().contains("Utf8"));
        assert!(err("table.fuzzy_join", &[tabular_contract(), right_contract()], json!({"left_key": "name", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "soundex", "blocking_param": 3})).to_string().contains("blocking_param"));
        // Collisione score: l'unica possibile e' una chiave sinistra chiamata
        // come la colonna score (la chiave conserva il nome, le altre colonne
        // prendono suffisso _L/_R).
        let key_named_score = DataContract::tabular(Arc::new(Schema::new(vec![Field::new(
            "score",
            DataType::Utf8,
            true,
        )])));
        assert!(err("table.fuzzy_join", &[key_named_score, right_contract()], json!({"left_key": "score", "right_key": "rname", "metric": "jaccard", "threshold": 0.5, "blocking": "prefix"})).to_string().contains("collisione"));
    }

    #[test]
    fn date_ops_produce_expected_types() {
        let formatted = ok(
            "table.date_format",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y-%m-%d", "output_column": "df"}),
        );
        assert_field(&formatted, "df", &DataType::Utf8, true);

        let added = ok(
            "table.date_add",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y-%m-%d", "amount": 3, "unit": "days", "output_column": "da"}),
        );
        assert_field(&added, "da", &DataType::Utf8, true);
        assert!(err(
            "table.date_add",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y", "amount": 1, "output_column": "da"})
        )
        .to_string()
        .contains("unit"));

        let diffed = ok(
            "table.date_diff",
            &[tabular_contract()],
            json!({"start_column": "name", "end_column": "name", "input_format": "%Y", "unit": "days", "output_column": "dd"}),
        );
        assert_field(&diffed, "dd", &DataType::Float64, true);

        let converted = ok(
            "table.timezone_convert",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Europe/Rome", "output_column": "tc"}),
        );
        assert_field(&converted, "tc", &DataType::Utf8, true);
        assert!(err(
            "table.timezone_convert",
            &[tabular_contract()],
            json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Marte/Olympus", "output_column": "tc"})
        )
        .to_string()
        .contains("timezone"));
    }

    // -- filtering / analysis -------------------------------------------------

    #[test]
    fn filter_preserves_schema_and_sorted_by_but_not_row_count() {
        let output = ok(
            "table.filter",
            &[proven_contract()],
            json!({"column": "value", "operator": ">", "value": 3}),
        );
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert_eq!(output.geometries.len(), 1, "filter preserva la geometria");
        assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
        assert!(output.properties.row_count.is_none());
        assert!(err(
            "table.filter",
            &[tabular_contract()],
            json!({"column": "value", "operator": ">", "value": "abc"})
        )
        .to_string()
        .contains("numeric"));
        assert!(err(
            "table.filter",
            &[tabular_contract()],
            json!({"column": "missing", "operator": "==", "value": 1})
        )
        .to_string()
        .contains("missing"));
    }

    #[test]
    fn conditional_type_comes_from_config_literals() {
        let numeric = ok(
            "table.conditional",
            &[tabular_contract()],
            json!({"column": "value", "conditions": [{"operator": ">", "value": 3, "result": 1}], "default_value": 0}),
        );
        assert_field(&numeric, "result", &DataType::Float64, true);
        let textual = ok(
            "table.conditional",
            &[tabular_contract()],
            json!({"column": "value", "conditions": [{"operator": ">", "value": 3, "result": "high"}], "default_value": "low"}),
        );
        assert_field(&textual, "result", &DataType::Utf8, false);
        assert!(err(
            "table.conditional",
            &[tabular_contract()],
            json!({"column": "missing", "conditions": []})
        )
        .to_string()
        .contains("missing"));
    }

    #[test]
    fn lookup_bin_statistics_sample_behave_as_kernels() {
        let lookup = ok(
            "table.lookup",
            &[tabular_contract()],
            json!({"column": "name", "mapping": {"a": "A"}}),
        );
        assert_field(&lookup, "name", &DataType::Utf8, true);
        assert!(err("table.lookup", &[tabular_contract()], json!({"column": "missing", "mapping": {}})).to_string().contains("missing"));

        let binned = ok("table.bin", &[tabular_contract()], json!({"column": "value", "bins": 5}));
        assert_field(&binned, "value_bin", &DataType::Utf8, true);
        assert!(err("table.bin", &[tabular_contract()], json!({"column": "value", "bins": 1})).to_string().contains("2..=100"));
        assert!(err(
            "table.bin",
            &[tabular_contract()],
            json!({"column": "value", "bins": [0.0, 10.0, 5.0]})
        )
        .to_string()
        .contains("crescenti"));

        let stats = ok("table.statistics", &[tabular_contract()], json!({"column": "value"}));
        for name in ["value_count", "value_min", "value_max", "value_mean", "value_median", "value_std"] {
            assert_field(&stats, name, &DataType::Float64, true);
        }
        assert!(err("table.statistics", &[tabular_contract()], json!({"column": "flag"})).to_string().contains("numero"));

        let sampled = ok("table.sample", &[proven_contract()], json!({"n": 10}));
        assert_eq!(sampled.schema.fields().len(), base_fields().len());
        assert_eq!(proven_rows(&sampled), 10, "sample senza stratify: min(n, righe)");
        assert!(sampled.properties.sorted_by.is_none(), "sample mescola le righe");
        assert!(err("table.sample", &[tabular_contract()], json!({"fraction": 1.5})).to_string().contains("fraction"));
    }

    #[test]
    fn flatten_json_requires_explicit_output_columns() {
        let output = ok(
            "table.flatten_json",
            &[tabular_contract()],
            json!({"column": "name", "output_columns": ["name_a", "name_b"]}),
        );
        assert_field(&output, "name_a", &DataType::Utf8, true);
        assert_field(&output, "name_b", &DataType::Utf8, true);
        assert!(matches!(
            err("table.flatten_json", &[tabular_contract()], json!({"column": "name"})),
            PlenoraError::Unsupported(_)
        ));
        assert!(err(
            "table.flatten_json",
            &[tabular_contract()],
            json!({"column": "name", "output_columns": ["other"]})
        )
        .to_string()
        .contains("prefix"));
    }

    // -- aggregation ----------------------------------------------------------

    #[test]
    fn sort_produces_proven_stream_sorted_by_and_keeps_rows() {
        let output = ok("table.sort", &[proven_contract()], json!({"columns": ["id", "name"]}));
        let keys = proven_sorted_keys(&output);
        assert_eq!(keys.len(), 2);
        assert_eq!(proven_rows(&output), 100);
        assert_eq!(output.geometries.len(), 1);
        assert!(err("table.sort", &[tabular_contract()], json!({"columns": []})).to_string().contains("columns"));
        assert!(err("table.sort", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));
    }

    #[test]
    fn distinct_and_dedup_keep_schema_and_drop_row_count() {
        let distinct = ok("table.distinct", &[proven_contract()], json!({"subset": ["name"]}));
        assert_eq!(distinct.schema.fields().len(), base_fields().len());
        assert!(distinct.properties.row_count.is_none());
        assert_eq!(proven_sorted_keys(&distinct), &[FieldId(0)]);
        assert!(err("table.distinct", &[tabular_contract()], json!({"subset": ["missing"]})).to_string().contains("missing"));

        let dedup = ok(
            "table.dedup_advanced",
            &[tabular_contract()],
            json!({"subset": ["name"], "order_column": "id"}),
        );
        assert_eq!(proven_sorted_keys(&dedup).len(), 1);
        assert!(matches!(
            err("table.dedup_advanced", &[tabular_contract()], json!({"subset": ["name"], "keep": "false"})),
            PlenoraError::Contract(_)
        ));
    }

    #[test]
    fn aggregate_builds_group_and_aggregation_columns() {
        let output = ok(
            "table.aggregate",
            &[tabular_contract()],
            json!({
                "group_by": ["name"],
                "aggregations": [
                    {"column": "value", "function": "sum"},
                    {"column": "value", "function": "mean"},
                    {"column": "id", "function": "count"}
                ]
            }),
        );
        assert_eq!(output.schema.fields().len(), 4);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_field(&output, "value_sum", &DataType::Float64, true);
        assert_field(&output, "value_mean", &DataType::Float64, true);
        assert_field(&output, "id", &DataType::Int64, false);
        assert!(output.geometries.is_empty(), "geometria non in group_by: tabellare");
        assert!(output.properties.sorted_by.is_none());
        assert!(output.properties.row_count.is_none());

        let count_only = ok(
            "table.aggregate",
            &[tabular_contract()],
            json!({"group_by": ["name"]}),
        );
        assert_field(&count_only, "count", &DataType::Int64, false);

        assert!(err("table.aggregate", &[tabular_contract()], json!({"group_by": []})).to_string().contains("group_by"));
        assert!(err(
            "table.aggregate",
            &[tabular_contract()],
            json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "quantile"}]})
        )
        .to_string()
        .contains("quantile"));
    }

    #[test]
    fn aggregate_preserves_geometry_grouped_by() {
        let output = ok(
            "table.aggregate",
            &[geo_contract()],
            json!({"group_by": ["geom"], "aggregations": [{"column": "id", "function": "count"}]}),
        );
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.geometries[0].name, "geom");
        assert_eq!(output.geometries[0].field_id, FieldId(7));
    }

    #[test]
    fn rolling_and_window_append_float64_and_track_order() {
        let rolling = ok(
            "table.rolling_window",
            &[tabular_contract()],
            json!({"column": "value", "function": "sum", "window": 3, "order_column": "id", "output_column": "rw"}),
        );
        assert_field(&rolling, "rw", &DataType::Float64, true);
        assert_eq!(proven_sorted_keys(&rolling).len(), 1);
        assert!(err(
            "table.rolling_window",
            &[tabular_contract()],
            json!({"column": "value", "function": "sum", "window": 0, "output_column": "rw"})
        )
        .to_string()
        .contains("window"));

        let window = ok(
            "table.window_function",
            &[tabular_contract()],
            json!({"column": "value", "function": "rank"}),
        );
        assert_field(&window, "value_rank", &DataType::Float64, true);
        assert!(err(
            "table.window_function",
            &[tabular_contract()],
            json!({"column": "value", "function": "ntile"})
        )
        .to_string()
        .contains("buckets"));
        assert!(err(
            "table.window_function",
            &[tabular_contract()],
            json!({"column": "value", "function": "rank", "buckets": 4})
        )
        .to_string()
        .contains("buckets"));
    }

    // -- columns / cleansing ---------------------------------------------------

    #[test]
    fn drop_columns_removes_geometry_and_becomes_tabular() {
        let output = ok("table.drop_columns", &[geo_contract()], json!({"columns": ["geom"]}));
        assert!(output.schema.field_with_name("geom").is_err());
        assert!(output.geometries.is_empty(), "drop della geometria -> tabellare");
        assert!(output.active_geometry.is_none());
        // Nomi inesistenti ignorati silenziosamente (comportamento del kernel).
        let unchanged = ok("table.drop_columns", &[geo_contract()], json!({"columns": ["nope"]}));
        assert_eq!(unchanged.geometries.len(), 1);
        assert!(err("table.drop_columns", &[tabular_contract()], json!({"columns": "x"})).to_string().contains("config"));
    }

    #[test]
    fn rename_preserves_field_id_and_changes_geometry_name() {
        let output = ok(
            "table.rename",
            &[geo_contract()],
            json!({"renames": [{"old_name": "geom", "new_name": "geometry"}]}),
        );
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.geometries[0].name, "geometry");
        assert_eq!(output.geometries[0].field_id, FieldId(7), "rinomina preserva il FieldId");
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        assert!(err(
            "table.rename",
            &[tabular_contract()],
            json!({"renames": [{"old_name": "id", "new_name": "name"}]})
        )
        .to_string()
        .contains("duplicato"));
    }

    #[test]
    fn reorder_columns_validates_and_reorders() {
        let output = ok(
            "table.reorder_columns",
            &[tabular_contract()],
            json!({"columns": ["name", "id"]}),
        );
        assert_eq!(output.schema.field(0).name(), "name");
        assert_eq!(output.schema.field(1).name(), "id");
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert!(err(
            "table.reorder_columns",
            &[tabular_contract()],
            json!({"columns": ["id", "id"]})
        )
        .to_string()
        .contains("ripetuta"));
        assert!(err(
            "table.reorder_columns",
            &[tabular_contract()],
            json!({"columns": ["missing"]})
        )
        .to_string()
        .contains("missing"));
    }

    #[test]
    fn concat_and_split_columns_validate_utf8() {
        let concatenated = ok(
            "table.concat_columns",
            &[tabular_contract()],
            json!({"columns": ["name"]}),
        );
        assert_field(&concatenated, "concatenated", &DataType::Utf8, true);
        assert!(err(
            "table.concat_columns",
            &[tabular_contract()],
            json!({"columns": ["name", "value"]})
        )
        .to_string()
        .contains("Utf8"));

        let split = ok(
            "table.split_column",
            &[tabular_contract()],
            json!({"column": "name", "new_columns": ["first", "second"]}),
        );
        assert_field(&split, "first", &DataType::Utf8, true);
        assert_field(&split, "second", &DataType::Utf8, true);
        assert_field(&split, "name", &DataType::Utf8, true);
        assert!(err(
            "table.split_column",
            &[tabular_contract()],
            json!({"column": "name", "new_columns": []})
        )
        .to_string()
        .contains("new_columns"));
    }

    #[test]
    fn fill_na_checks_types_and_value_coherence() {
        let output = ok(
            "table.fill_na",
            &[tabular_contract()],
            json!({"column": "value", "value": 0}),
        );
        assert_field(&output, "value", &DataType::Float64, true);
        assert!(err(
            "table.fill_na",
            &[tabular_contract()],
            json!({"column": "value", "value": "abc"})
        )
        .to_string()
        .contains("fill"));
        // Senza column il target e' l'intero schema: Binary (geometria) non supportato.
        assert!(err("table.fill_na", &[geo_contract()], json!({}))
            .to_string()
            .contains("fill_na non supporta"));
    }

    #[test]
    fn replace_requires_utf8_and_valid_regex() {
        let output = ok(
            "table.replace",
            &[tabular_contract()],
            json!({"column": "name", "old_value": "a", "new_value": "b"}),
        );
        assert_field(&output, "name", &DataType::Utf8, true);
        assert!(err(
            "table.replace",
            &[tabular_contract()],
            json!({"column": "value", "old_value": "1", "new_value": "2"})
        )
        .to_string()
        .contains("Utf8"));
        assert!(err(
            "table.replace",
            &[tabular_contract()],
            json!({"column": "name", "old_value": "(", "new_value": "b", "regex": true})
        )
        .to_string()
        .contains("regex"));
    }

    #[test]
    fn type_cast_maps_target_types_and_drops_geometry_metadata() {
        let output = ok(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "date32"}),
        );
        assert_field(&output, "name", &DataType::Date32, true);

        let decimal = ok(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "decimal128", "precision": 12, "scale": 3}),
        );
        assert_field(&decimal, "name", &DataType::Decimal128(12, 3), true);

        let timestamp = ok(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "timestamp_millis", "timezone": "UTC"}),
        );
        assert_field(
            &timestamp,
            "name",
            &DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
            true,
        );

        // Cast della colonna geometrica: metadati geoarrow persi -> tabellare.
        let casted_geometry = ok(
            "table.type_cast",
            &[geo_contract()],
            json!({"column": "geom", "target_type": "str"}),
        );
        assert!(casted_geometry.geometries.is_empty());

        assert!(err(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "decimal128"})
        )
        .to_string()
        .contains("precision"));
        assert!(err(
            "table.type_cast",
            &[tabular_contract()],
            json!({"column": "name", "target_type": "timestamp_millis", "timezone": "Marte/Olympus"})
        )
        .to_string()
        .contains("timezone"));
    }

    // -- formula / expression ---------------------------------------------------

    #[test]
    fn formula_infers_static_type() {
        let numeric = ok(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "value * 2 + id"}),
        );
        assert_field(&numeric, "f", &DataType::Float64, true);
        let textual = ok(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "name + ' suffix'"}),
        );
        assert_field(&textual, "f", &DataType::Utf8, true);
        assert!(err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "value +"})
        )
        .to_string()
        .contains("formula"));
        assert!(err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "missing + 1"})
        )
        .to_string()
        .contains("missing"));
        assert!(err(
            "table.formula",
            &[tabular_contract()],
            json!({"new_column": "f", "formula": "name - 'x'"})
        )
        .to_string()
        .contains("testo"));
    }

    #[test]
    fn expression_uses_declared_or_inferred_output_type() {
        let declared = ok(
            "table.expression",
            &[tabular_contract()],
            json!({
                "output_column": "e",
                "expression": {"kind": "column", "name": "name"},
                "output_type": "boolean"
            }),
        );
        assert_field(&declared, "e", &DataType::Boolean, true);

        let inferred = ok(
            "table.expression",
            &[tabular_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "binary",
                    "op": "add",
                    "left": {"kind": "column", "name": "value"},
                    "right": {"kind": "literal", "value": 1}
                }
            }),
        );
        assert_field(&inferred, "e", &DataType::Float64, true);

        let heterogeneous = err(
            "table.expression",
            &[tabular_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "function",
                    "name": "coalesce",
                    "args": [{"kind": "column", "name": "value"}, {"kind": "column", "name": "name"}]
                }
            }),
        );
        assert!(heterogeneous.to_string().contains("eterogenei"), "{heterogeneous}");
        assert!(err(
            "table.expression",
            &[tabular_contract()],
            json!({"output_column": "e", "expression": {"kind": "column", "name": "missing"}})
        )
        .to_string()
        .contains("missing"));
    }

    /// Contratto con colonne temporali native per `date_trunc`.
    fn temporal_contract() -> DataContract {
        DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("d", DataType::Date32, true),
            Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new(
                "tstz",
                DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                true,
            ),
            Field::new("name", DataType::Utf8, true),
        ])))
    }

    fn date_trunc(unit: &str, column: &str) -> Value {
        json!({
            "kind": "function",
            "name": "date_trunc",
            "args": [{"kind": "literal", "value": unit}, {"kind": "column", "name": column}]
        })
    }

    #[test]
    fn expression_date_trunc_native_temporal_types() {
        // Auto: il tipo discende dalla colonna di input (mai Utf8).
        let date = ok(
            "table.expression",
            &[temporal_contract()],
            json!({"output_column": "e", "expression": date_trunc("month", "d")}),
        );
        assert_field(&date, "e", &DataType::Date32, true);
        let ts = ok(
            "table.expression",
            &[temporal_contract()],
            json!({"output_column": "e", "expression": date_trunc("hour", "ts")}),
        );
        assert_field(
            &ts,
            "e",
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        );
        // output_type esplicito date32/timestamp_ms.
        let explicit = ok(
            "table.expression",
            &[temporal_contract()],
            json!({
                "output_column": "e",
                "expression": date_trunc("day", "ts"),
                "output_type": "timestamp_ms"
            }),
        );
        assert_field(
            &explicit,
            "e",
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        );
        // Annidamento: il tipo discende dalla colonna radice.
        let nested = ok(
            "table.expression",
            &[temporal_contract()],
            json!({
                "output_column": "e",
                "expression": {
                    "kind": "function",
                    "name": "date_trunc",
                    "args": [{"kind": "literal", "value": "year"}, date_trunc("month", "ts")]
                }
            }),
        );
        assert_field(
            &nested,
            "e",
            &DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        );
        // Unita' invalida, non letterale, sub-day su Date32.
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("week", "ts")})
            )
            .to_string()
            .contains("unita' non valida")
        );
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({
                    "output_column": "e",
                    "expression": {
                        "kind": "function",
                        "name": "date_trunc",
                        "args": [{"kind": "column", "name": "name"}, {"kind": "column", "name": "ts"}]
                    }
                })
            )
            .to_string()
            .contains("letterale")
        );
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("hour", "d")})
            )
            .to_string()
            .contains("sub-day")
        );
        // Input testuale (nessun parsing) e timezone-aware: errori in validazione.
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("day", "name")})
            )
            .to_string()
            .contains("Date32 o Timestamp")
        );
        assert!(
            err(
                "table.expression",
                &[temporal_contract()],
                json!({"output_column": "e", "expression": date_trunc("day", "tstz")})
            )
            .to_string()
            .contains("timezone-aware")
        );
    }

    // -- quality / governance ----------------------------------------------------

    #[test]
    fn assert_ops_validate_and_pass_through() {
        let input = proven_contract();
        let output = ok(
            "table.assert_schema",
            std::slice::from_ref(&input),
            json!({"fields": [
                {"name": "id", "data_type": "int64", "nullable": false},
                {"name": "name", "data_type": "utf8"}
            ], "allow_extra": true}),
        );
        assert_eq!(proven_rows(&output), 100);
        assert!(err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "id", "data_type": "utf8"}], "allow_extra": true})
        )
        .to_string()
        .contains("tipo errato"));
        assert!(err(
            "table.assert_schema",
            &[tabular_contract()],
            json!({"fields": [{"name": "id", "data_type": "wat"}], "allow_extra": true})
        )
        .to_string()
        .contains("non supportato"));

        assert!(ok("table.assert_not_null", &[tabular_contract()], json!({"columns": ["id"]})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_not_null", &[tabular_contract()], json!({"columns": ["missing"]})).to_string().contains("missing"));

        assert!(ok("table.assert_unique", &[tabular_contract()], json!({"columns": ["id"]})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_unique", &[tabular_contract()], json!({"columns": ["st"]})).to_string().contains("scalare"));

        assert!(ok("table.assert_range", &[tabular_contract()], json!({"column": "value", "min": 0})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_range", &[tabular_contract()], json!({"column": "flag"})).to_string().contains("numero"));

        assert!(ok("table.assert_regex", &[tabular_contract()], json!({"column": "name", "pattern": "^a+$"})).schema.fields().len() == base_fields().len());
        assert!(err("table.assert_regex", &[tabular_contract()], json!({"column": "name", "pattern": "("})).to_string().contains("regex"));
        assert!(err("table.assert_regex", &[tabular_contract()], json!({"column": "value", "pattern": "a"})).to_string().contains("Utf8"));

        let coalesced = ok(
            "table.coalesce",
            &[tabular_contract()],
            json!({"columns": ["value"], "output_column": "c"}),
        );
        assert_field(&coalesced, "c", &DataType::Float64, true);
        assert!(err(
            "table.coalesce",
            &[tabular_contract()],
            json!({"columns": ["value", "id"], "output_column": "c"})
        )
        .to_string()
        .contains("identici"));
        assert!(err(
            "table.coalesce",
            &[tabular_contract()],
            json!({"columns": [], "output_column": "c"})
        )
        .to_string()
        .contains("almeno una"));
    }

    #[test]
    fn assert_cardinality_uses_proven_row_count() {
        assert!(ok(
            "table.assert_cardinality",
            &[proven_contract()],
            json!({"min_rows": 50, "max_rows": 100})
        )
        .schema
        .fields()
        .len()
            == base_fields().len());
        assert!(matches!(
            err("table.assert_cardinality", &[proven_contract()], json!({"exact_rows": 5})),
            PlenoraError::Contract(_)
        ));
    }

    #[test]
    fn assert_metadata_checks_schema_metadata() {
        let mut input = tabular_contract();
        input.schema = Arc::new(Schema::new_with_metadata(
            base_fields(),
            std::collections::HashMap::from([("source".to_owned(), "test".to_owned())]),
        ));
        assert!(ok(
            "table.assert_metadata",
            std::slice::from_ref(&input),
            json!({"expected": {"source": "test"}})
        )
        .schema
        .metadata()
        .contains_key("source"));
        assert!(err(
            "table.assert_metadata",
            &[input],
            json!({"expected": {"source": "other"}})
        )
        .to_string()
        .contains("metadata"));
    }

    #[test]
    fn assert_foreign_key_and_reconcile_are_binary() {
        let left = tabular_contract();
        let right = right_contract();
        let output = ok(
            "table.assert_foreign_key",
            &[left.clone(), right.clone()],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_eq!(output.schema.fields().len(), base_fields().len());
        assert!(err(
            "table.assert_foreign_key",
            &[left.clone(), right.clone()],
            json!({"left_keys": ["id"], "right_keys": ["rname"]})
        )
        .to_string()
        .contains("tipi"));

        let report = ok(
            "table.reconcile",
            &[left, right],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_eq!(report.schema.fields().len(), 2);
        assert_field(&report, "metric", &DataType::Utf8, false);
        assert_field(&report, "value", &DataType::UInt64, false);
        assert_eq!(proven_rows(&report), 5);
        assert!(report.geometries.is_empty());
    }

    // -- reshape ------------------------------------------------------------------

    #[test]
    fn melt_builds_id_var_value_columns() {
        assert!(err(
            "table.melt",
            &[tabular_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value", "dc"]})
        )
        .to_string()
        .contains("eterogenee"));
        let homogeneous = ok(
            "table.melt",
            &[proven_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value"]}),
        );
        let fields = &homogeneous.schema;
        assert_eq!(fields.field(0).name(), "id");
        assert_field(&homogeneous, "variable", &DataType::Utf8, false);
        // collision_free del kernel: "value" esiste gia' nell'input -> "value_1".
        assert_field(&homogeneous, "value_1", &DataType::Float64, true);
        assert_eq!(proven_rows(&homogeneous), 100);

        let as_string = ok(
            "table.melt",
            &[tabular_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value", "dc"], "type_policy": "string"}),
        );
        assert_field(&as_string, "value_1", &DataType::Utf8, true);

        // Collisione col nome di una colonna esistente: suffisso automatico.
        let collision = ok(
            "table.melt",
            &[tabular_contract()],
            json!({"id_columns": ["id"], "value_columns": ["value"], "var_name": "name"}),
        );
        assert_field(&collision, "name_1", &DataType::Utf8, false);

        // Geometria come colonna id: preservata.
        let with_geometry = ok(
            "table.melt",
            &[geo_contract()],
            json!({"id_columns": ["geom"], "value_columns": ["value"]}),
        );
        assert_eq!(with_geometry.geometries.len(), 1);
        assert_eq!(with_geometry.geometries[0].field_id, FieldId(7));
    }

    #[test]
    fn pivot_and_transpose_are_explicitly_unsupported() {
        let pivot = err(
            "table.pivot",
            &[tabular_contract()],
            json!({"index_col": "id", "pivot_col": "name", "value_col": "value"}),
        );
        assert!(matches!(pivot, PlenoraError::Unsupported(_)), "{pivot}");
        let transpose = err(
            "table.transpose",
            &[tabular_contract()],
            json!({"id_column": "id"}),
        );
        assert!(matches!(transpose, PlenoraError::Unsupported(_)), "{transpose}");
        // Config invalida fallisce prima come Contract.
        assert!(matches!(
            err("table.pivot", &[tabular_contract()], json!({"index_col": 1})),
            PlenoraError::Contract(_)
        ));
    }

    #[test]
    fn explode_replaces_list_with_element_type() {
        let output = ok("table.explode", &[geo_contract()], json!({"column": "lst"}));
        assert_field(&output, "lst", &DataType::Int64, true);
        assert_eq!(output.geometries.len(), 1);
        assert!(output.properties.row_count.is_none());
        let renamed = ok(
            "table.explode",
            &[tabular_contract()],
            json!({"column": "lst", "output_column": "element"}),
        );
        assert_field(&renamed, "element", &DataType::Int64, true);
        assert_field(&renamed, "lst", &DataType::List(Arc::new(Field::new("item", DataType::Int64, true))), true);
        assert!(err("table.explode", &[tabular_contract()], json!({"column": "id"})).to_string().contains("List"));
    }

    #[test]
    fn unnest_expands_struct_fields() {
        let output = ok("table.unnest", &[geo_contract()], json!({"column": "st"}));
        assert!(output.schema.field_with_name("st").is_err(), "drop_source di default");
        assert_field(&output, "a", &DataType::Int64, true);
        assert_field(&output, "b", &DataType::Utf8, true);
        assert_eq!(output.geometries.len(), 1);
        let prefixed = ok(
            "table.unnest",
            &[tabular_contract()],
            json!({"column": "st", "prefix": "s_", "drop_source": false}),
        );
        assert_field(&prefixed, "s_a", &DataType::Int64, true);
        assert_field(&prefixed, "st", &DataType::Struct(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Utf8, true),
        ].into()), true);
        assert!(err("table.unnest", &[tabular_contract()], json!({"column": "id"})).to_string().contains("Struct"));
        // Nome derivato che collide con una colonna esistente: errore.
        let colliding = DataContract::tabular(Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int64, true),
            Field::new(
                "st",
                DataType::Struct(vec![Field::new("a", DataType::Int64, true)].into()),
                true,
            ),
        ])));
        assert!(err("table.unnest", &[colliding], json!({"column": "st"}))
            .to_string()
            .contains("collisione"));
    }

    #[test]
    fn table_diff_builds_key_compare_and_diff_columns() {
        let output = ok(
            "table.table_diff",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        // compare di default: colonne left non chiave presenti in right.
        assert_field(&output, "id", &DataType::Int64, true);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_field(&output, "value", &DataType::Float64, true);
        assert_field(&output, "_diff_status", &DataType::Utf8, false);
        assert_field(&output, "_diff_columns", &DataType::Utf8, true);
        assert_field(&output, "_diff_old_values", &DataType::Utf8, true);
        assert_eq!(output.schema.fields().len(), 6);
        assert!(err(
            "table.table_diff",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": [], "right_keys": []})
        )
        .to_string()
        .contains("chiavi"));
        assert!(err(
            "table.table_diff",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rid"], "compare_columns": ["nope"]})
        )
        .to_string()
        .contains("nope"));
    }

    // -- joins / setops ------------------------------------------------------------

    #[test]
    fn join_suffixes_columns_and_forces_nullable() {
        let output = ok(
            "table.join",
            &[geo_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rid"]}),
        );
        assert_field(&output, "id", &DataType::Int64, true);
        assert_field(&output, "name_L", &DataType::Utf8, true);
        assert_field(&output, "value_L", &DataType::Float64, true);
        assert_field(&output, "name_R", &DataType::Utf8, true);
        assert_field(&output, "rname_R", &DataType::Utf8, true);
        assert!(output.schema.field_with_name("rid").is_err(), "chiave right omessa");
        // Geometria left non chiave: rinominata _L, FieldId preservato.
        assert_eq!(output.geometries.len(), 1);
        assert_eq!(output.geometries[0].name, "geom_L");
        assert_eq!(output.geometries[0].field_id, FieldId(7));
        assert_eq!(output.active_geometry, Some(FieldId(7)));
        for field in output.schema.fields() {
            assert!(field.is_nullable(), "join forza nullable=true su {}", field.name());
        }
        assert!(err(
            "table.join",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": ["rname"]})
        )
        .to_string()
        .contains("tipi"));
        assert!(err(
            "table.join",
            &[tabular_contract(), right_contract()],
            json!({"left_keys": ["id"], "right_keys": []})
        )
        .to_string()
        .contains("chiavi"));
    }

    #[test]
    fn join_with_two_geometries_fails_closed() {
        let mut right = right_contract();
        let mut fields: Vec<Field> = right
            .schema
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect();
        fields.push(Field::new("rgeom", DataType::Binary, true));
        right = DataContract::new(
            Arc::new(Schema::new(fields)),
            vec![geometry(70, "rgeom", true)],
            Some(FieldId(70)),
            ContractProperties::default(),
        )
        .unwrap();
        assert!(matches!(
            err(
                "table.join",
                &[geo_contract(), right],
                json!({"left_keys": ["id"], "right_keys": ["rid"]})
            ),
            PlenoraError::Contract(_)
        ));
    }

    #[test]
    fn cross_join_uses_pandas_suffixes() {
        let output = ok(
            "table.cross_join",
            &[tabular_contract(), right_contract()],
            json!({}),
        );
        assert_field(&output, "name_x", &DataType::Utf8, true);
        assert_field(&output, "name_y", &DataType::Utf8, true);
        assert_field(&output, "value_x", &DataType::Float64, true);
        assert_field(&output, "value_y", &DataType::Float64, true);
        assert_field(&output, "rid", &DataType::Int64, true);
        assert_field(&output, "rname", &DataType::Utf8, true);
        // Collisione residua dopo il rename: errore.
        let mut colliding_fields = base_fields();
        colliding_fields.push(Field::new("name_x", DataType::Utf8, true));
        let colliding = DataContract::tabular(Arc::new(Schema::new(colliding_fields)));
        assert!(err("table.cross_join", &[colliding, right_contract()], json!({}))
            .to_string()
            .contains("collisione"));
    }

    #[test]
    fn membership_joins_return_left_unchanged() {
        for op in ["table.semi_join", "table.anti_join"] {
            let output = ok(
                op,
                &[proven_contract(), right_contract()],
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
            );
            assert_eq!(output.schema.fields().len(), base_fields().len());
            assert_eq!(output.geometries.len(), 1);
            assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
            assert!(output.properties.row_count.is_none());
            assert!(err(op, &[tabular_contract(), right_contract()], json!({"left_keys": [], "right_keys": []}))
                .to_string()
                .contains("chiavi"));
        }
    }

    #[test]
    fn asof_join_keeps_left_rows_and_appends_right_columns() {
        let output = ok(
            "table.asof_join",
            &[proven_contract(), right_contract()],
            json!({"left_on": "id", "right_on": "rid"}),
        );
        assert_field(&output, "id", &DataType::Int64, true);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_field(&output, "name_R", &DataType::Utf8, true);
        assert_field(&output, "value_R", &DataType::Float64, true);
        assert_field(&output, "rname", &DataType::Utf8, true);
        assert!(output.schema.field_with_name("rid").is_err());
        assert_eq!(proven_rows(&output), 100, "una riga per riga left");
        assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
        assert!(err(
            "table.asof_join",
            &[tabular_contract(), right_contract()],
            json!({"left_on": "name", "right_on": "rname"})
        )
        .to_string()
        .contains("Int64 o Float64"));
        assert!(err(
            "table.asof_join",
            &[tabular_contract(), right_contract()],
            json!({"left_on": "id", "right_on": "rid", "tolerance": -1.0})
        )
        .to_string()
        .contains("tolerance"));
    }

    #[test]
    fn concat_merges_nullability_and_sums_row_count() {
        let inputs: [DataContract; 2] = simple_pair().into();
        let output = ok("table.concat", &inputs, json!({}));
        assert_field(&output, "id", &DataType::Int64, false);
        assert_field(&output, "name", &DataType::Utf8, true);
        assert_eq!(proven_rows(&output), 150);
        assert!(output.properties.sorted_by.is_none());
        assert!(err(
            "table.concat",
            &[simple_pair().0, right_contract()],
            json!({})
        )
        .to_string()
        .contains("schemi"));
    }

    #[test]
    fn set_operations_validate_schema_and_encoder_types() {
        let (left, right) = simple_pair();
        let union = ok("table.union_distinct", &[left.clone(), right.clone()], json!({}));
        assert_field(&union, "name", &DataType::Utf8, true);
        assert!(union.properties.row_count.is_none());

        for op in ["table.intersect", "table.except"] {
            let output = ok(op, &[left.clone(), right.clone()], json!({}));
            assert_eq!(output.schema.fields().len(), 2);
            assert!(output.properties.row_count.is_none());
            assert!(output.properties.sorted_by.is_none());
        }
        // Schema con List/Struct: l'encoder delle chiavi non li supporta.
        assert!(err(
            "table.union_distinct",
            &[tabular_contract(), tabular_contract()],
            json!({})
        )
        .to_string()
        .contains("non supportato"));
        assert!(err(
            "table.intersect",
            &[simple_pair().0, right_contract()],
            json!({})
        )
        .to_string()
        .contains("schemi"));
    }

    // -- Proprieta' e geometria ----------------------------------------------------

    #[test]
    fn append_ops_preserve_row_count_and_sorted_by() {
        let output = ok("table.uuid_generator", &[proven_contract()], json!({}));
        assert_eq!(proven_rows(&output), 100);
        assert_eq!(proven_sorted_keys(&output), &[FieldId(0)]);
        assert_eq!(output.geometries.len(), 1);
    }

    #[test]
    fn overwriting_a_column_drops_sorted_by() {
        // lookup di default sovrascrive la colonna sorgente: possibile chiave.
        let output = ok(
            "table.lookup",
            &[proven_contract()],
            json!({"column": "name", "mapping": {"a": "A"}}),
        );
        assert!(output.properties.sorted_by.is_none());
        assert_eq!(proven_rows(&output), 100);
    }

    #[test]
    fn field_allocator_avoids_collisions_with_input_ids() {
        let mut allocator = FieldAllocator::default();
        let fresh = allocator.alloc();
        assert_eq!(fresh, FieldId(0));
        // L'allocatore osserva gli id degli input (geometria FieldId(7),
        // chiavi FieldId(0)) prima di internare.
        let _ = ok_with_allocator(&mut allocator);
        let id = allocator.alloc();
        assert!(id.0 > 7, "alloc dopo observe deve superare gli id di input: {id}");
    }

    fn ok_with_allocator(allocator: &mut FieldAllocator) -> DataContract {
        analyze_table_contract(
            "table.sort",
            &[proven_contract()],
            &json!({"columns": ["id"]}),
            allocator,
        )
        .unwrap()
    }

    #[test]
    fn sample_and_sort_keep_geometry() {
        let sampled = ok("table.sample", &[geo_contract()], json!({"n": 5}));
        assert_eq!(sampled.geometries.len(), 1);
        let sorted = ok("table.sort", &[geo_contract()], json!({"columns": ["id"]}));
        assert_eq!(sorted.geometries.len(), 1);
        assert_eq!(sorted.geometries[0].field_id, FieldId(7));
    }

    #[test]
    fn config_with_unknown_fields_fails_closed() {
        assert!(matches!(
            err(
                "table.filter",
                &[tabular_contract()],
                json!({"column": "value", "operator": "==", "value": 1, "bogus": true})
            ),
            PlenoraError::Contract(_)
        ));
    }

    // -- Cross-check analyze vs kernel su batch reali --------------------------
    //
    // Esegue il kernel vero su un piccolo RecordBatch e confronta lo schema
    // del batch prodotto con quello inferito a secco da analyze_table_contract
    // (nomi, tipi, nullability). Copre tutte le op con schema deterministico.

    mod kernel_crosscheck {
        use plenora_core::arrow::array::{
            types::Int64Type, ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array,
            ListArray, RecordBatch, StringArray, StructArray,
        };

        use super::*;

        fn signature(schema: &Schema) -> Vec<(String, DataType, bool)> {
            schema
                .fields()
                .iter()
                .map(|field| {
                    (
                        field.name().clone(),
                        field.data_type().clone(),
                        field.is_nullable(),
                    )
                })
                .collect()
        }

        fn simple_batch() -> RecordBatch {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, true),
                    Field::new("value", DataType::Float64, true),
                    Field::new("flag", DataType::Boolean, true),
                    Field::new("geom", DataType::Binary, true),
                ])),
                vec![
                    Arc::new(Int64Array::from(vec![3, 1, 2])),
                    Arc::new(StringArray::from(vec![Some("b"), Some("a"), None])),
                    Arc::new(Float64Array::from(vec![Some(3.5), Some(1.5), None])),
                    Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
                    Arc::new(BinaryArray::from(vec![
                        Some(b"wkb-a".as_slice()),
                        Some(b"wkb-b".as_slice()),
                        None,
                    ])),
                ],
            )
            .unwrap()
        }

        fn nested_batch() -> RecordBatch {
            let list: ArrayRef = Arc::new(ListArray::from_iter_primitive::<Int64Type, _, _>(vec![
                Some(vec![Some(1), Some(2)]),
                Some(vec![]),
                Some(vec![Some(3)]),
            ]));
            let structure: ArrayRef = Arc::new(StructArray::from(vec![
                (
                    Arc::new(Field::new("a", DataType::Int64, true)),
                    Arc::new(Int64Array::from(vec![Some(1), Some(2), None])) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("b", DataType::Utf8, true)),
                    Arc::new(StringArray::from(vec![Some("x"), None, Some("z")])) as ArrayRef,
                ),
            ]));
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("lst", list.data_type().clone(), true),
                    Field::new("st", structure.data_type().clone(), true),
                ])),
                vec![
                    Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
                    list,
                    structure,
                ],
            )
            .unwrap()
        }

        fn right_batch() -> RecordBatch {
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("rid", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, true),
                    Field::new("value", DataType::Float64, true),
                    Field::new("rname", DataType::Utf8, true),
                ])),
                vec![
                    Arc::new(Int64Array::from(vec![1, 2, 3])),
                    Arc::new(StringArray::from(vec![Some("a"), Some("b"), None])),
                    Arc::new(Float64Array::from(vec![Some(10.0), Some(20.0), None])),
                    Arc::new(StringArray::from(vec![Some("x"), Some("y"), None])),
                ],
            )
            .unwrap()
        }

        fn geo_input(batch: &RecordBatch) -> DataContract {
            DataContract::new(
                batch.schema(),
                vec![geometry(1, "geom", true)],
                Some(FieldId(1)),
                ContractProperties::default(),
            )
            .unwrap()
        }

        #[allow(clippy::needless_pass_by_value)]
        fn check_unary(
            op: &str,
            batch: &RecordBatch,
            config: Value,
            kernel: impl FnOnce(&RecordBatch) -> Result<RecordBatch>,
        ) -> DataContract {
            let expected = kernel(batch).unwrap_or_else(|e| panic!("kernel {op}: {e}"));
            let input = geo_input(batch);
            let analyzed = analyze_table_contract(op, &[input], &config, &mut FieldAllocator::default())
                .unwrap_or_else(|e| panic!("analyze {op}: {e}"));
            assert_eq!(
                signature(&analyzed.schema),
                signature(&expected.schema()),
                "schema diverso per {op}"
            );
            analyzed
        }

        #[allow(clippy::needless_pass_by_value)]
        fn check_unary_plain(
            op: &str,
            batch: &RecordBatch,
            config: Value,
            kernel: impl FnOnce(&RecordBatch) -> Result<RecordBatch>,
        ) {
            let expected = kernel(batch).unwrap_or_else(|e| panic!("kernel {op}: {e}"));
            let input = DataContract::tabular(batch.schema());
            let analyzed = analyze_table_contract(op, &[input], &config, &mut FieldAllocator::default())
                .unwrap_or_else(|e| panic!("analyze {op}: {e}"));
            assert_eq!(
                signature(&analyzed.schema),
                signature(&expected.schema()),
                "schema diverso per {op}"
            );
        }

        #[allow(clippy::needless_pass_by_value)]
        fn check_binary(
            op: &str,
            left: &RecordBatch,
            right: &RecordBatch,
            config: Value,
            kernel: impl FnOnce(&RecordBatch, &RecordBatch) -> Result<RecordBatch>,
        ) {
            let expected = kernel(left, right).unwrap_or_else(|e| panic!("kernel {op}: {e}"));
            let inputs = [geo_input(left), DataContract::tabular(right.schema())];
            let analyzed =
                analyze_table_contract(op, &inputs, &config, &mut FieldAllocator::default())
                    .unwrap_or_else(|e| panic!("analyze {op}: {e}"));
            assert_eq!(
                signature(&analyzed.schema),
                signature(&expected.schema()),
                "schema diverso per {op}"
            );
        }

        fn cfg<T: serde::de::DeserializeOwned>(config: &Value) -> T {
            serde_json::from_value(config.clone()).unwrap()
        }

        #[test]
        // Un cross-check per op: lunghezza intrinseca.
        #[allow(clippy::too_many_lines)]
        fn unary_ops_match_kernel_schemas() {
            let batch = simple_batch();
            let limits = Limits::default();

            check_unary("table.add_row_number", &batch, json!({}), |b| {
                utility::add_row_number(b, &cfg(&json!({})))
            });
            check_unary("table.sort", &batch, json!({"columns": ["id"]}), |b| {
                aggregation::sort(b, &cfg(&json!({"columns": ["id"]})))
            });
            check_unary("table.distinct", &batch, json!({}), |b| {
                aggregation::distinct(b, &cfg(&json!({})))
            });
            check_unary(
                "table.dedup_advanced",
                &batch,
                json!({"subset": ["name"]}),
                |b| aggregation::dedup_advanced(b, &cfg(&json!({"subset": ["name"]}))),
            );
            let filtered = check_unary(
                "table.filter",
                &batch,
                json!({"column": "value", "operator": ">", "value": 2}),
                |b| filtering::filter(b, &cfg(&json!({"column": "value", "operator": ">", "value": 2}))),
            );
            assert_eq!(filtered.geometries.len(), 1);
            check_unary("table.sample", &batch, json!({"n": 2}), |b| {
                analysis::sample(b, &cfg(&json!({"n": 2})))
            });
            check_unary(
                "table.rename",
                &batch,
                json!({"renames": [{"old_name": "name", "new_name": "label"}]}),
                |b| {
                    columns::rename(
                        b,
                        &cfg(&json!({"renames": [{"old_name": "name", "new_name": "label"}]})),
                    )
                },
            );
            check_unary("table.drop_columns", &batch, json!({"columns": ["flag"]}), |b| {
                columns::drop_columns(b, &cfg(&json!({"columns": ["flag"]})))
            });
            check_unary(
                "table.reorder_columns",
                &batch,
                json!({"columns": ["name"]}),
                |b| columns::reorder_columns(b, &cfg(&json!({"columns": ["name"]}))),
            );
            check_unary("table.concat_columns", &batch, json!({"columns": ["name"]}), |b| {
                columns::concat_columns(b, &cfg(&json!({"columns": ["name"]})), &limits)
            });
            check_unary(
                "table.split_column",
                &batch,
                json!({"column": "name", "new_columns": ["first"]}),
                |b| {
                    columns::split_column(
                        b,
                        &cfg(&json!({"column": "name", "new_columns": ["first"]})),
                        &limits,
                    )
                },
            );
            check_unary(
                "table.fill_na",
                &batch,
                json!({"column": "name", "value": "?"}),
                |b| cleansing::fill_na(b, &cfg(&json!({"column": "name", "value": "?"}))),
            );
            check_unary(
                "table.replace",
                &batch,
                json!({"column": "name", "old_value": "a", "new_value": "z"}),
                |b| {
                    cleansing::replace(
                        b,
                        &cfg(&json!({"column": "name", "old_value": "a", "new_value": "z"})),
                    )
                },
            );
            check_unary(
                "table.type_cast",
                &batch,
                json!({"column": "id", "target_type": "str"}),
                |b| cleansing::type_cast(b, &cfg(&json!({"column": "id", "target_type": "str"}))),
            );
            check_unary(
                "table.conditional",
                &batch,
                json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": 1}], "default_value": 0}),
                |b| {
                    filtering::conditional(
                        b,
                        &cfg(&json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": 1}], "default_value": 0})),
                    )
                },
            );
            check_unary(
                "table.conditional",
                &batch,
                json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": "hi"}], "default_value": "lo"}),
                |b| {
                    filtering::conditional(
                        b,
                        &cfg(&json!({"column": "value", "conditions": [{"operator": ">", "value": 2, "result": "hi"}], "default_value": "lo"})),
                    )
                },
            );
            check_unary(
                "table.lookup",
                &batch,
                json!({"column": "name", "mapping": {"a": "A"}}),
                |b| analysis::lookup(b, &cfg(&json!({"column": "name", "mapping": {"a": "A"}}))),
            );
            check_unary("table.bin", &batch, json!({"column": "value", "bins": 2}), |b| {
                analysis::bin(b, &cfg(&json!({"column": "value", "bins": 2})))
            });
            check_unary("table.statistics", &batch, json!({"column": "value"}), |b| {
                analysis::statistics(b, &cfg(&json!({"column": "value"})))
            });
            check_unary(
                "table.date_extract",
                &batch,
                json!({"column": "name", "parts": ["year", "month"]}),
                |b| {
                    utility::date_extract(
                        b,
                        &cfg(&json!({"column": "name", "parts": ["year", "month"]})),
                    )
                },
            );
            check_unary("table.string_pad", &batch, json!({"column": "name"}), |b| {
                strings::string_pad(b, &cfg(&json!({"column": "name"})), &limits)
            });
            check_unary("table.string_length", &batch, json!({"column": "name"}), |b| {
                strings::string_length(b, &cfg(&json!({"column": "name"})))
            });
            check_unary(
                "table.string_extract",
                &batch,
                json!({"column": "name", "pattern": "(?P<x>a)"}),
                |b| {
                    strings::string_extract(
                        b,
                        &cfg(&json!({"column": "name", "pattern": "(?P<x>a)"})),
                        &limits,
                    )
                },
            );
            check_unary("table.text_normalize", &batch, json!({"columns": ["name"]}), |b| {
                strings::text_normalize(b, &cfg(&json!({"columns": ["name"]})), &limits)
            });
            check_unary("table.md5_hash", &batch, json!({"columns": ["name"]}), |b| {
                security::md5_hash(b, &cfg(&json!({"columns": ["name"]})))
            });
            check_unary("table.sha256_hash", &batch, json!({"columns": ["name"]}), |b| {
                security::sha256_hash(b, &cfg(&json!({"columns": ["name"]})))
            });
            check_unary(
                "table.mask_data",
                &batch,
                json!({"maskings": [{"column": "name"}]}),
                |b| security::mask_data(b, &cfg(&json!({"maskings": [{"column": "name"}]}))),
            );
            check_unary("table.uuid_generator", &batch, json!({}), |b| {
                utility::uuid_generator(b, &cfg(&json!({})))
            });
            check_unary(
                "table.date_format",
                &batch,
                json!({"column": "name", "input_format": "%Y", "output_column": "df"}),
                |b| {
                    dates::date_format(
                        b,
                        &cfg(&json!({"column": "name", "input_format": "%Y", "output_column": "df"})),
                    )
                },
            );
            check_unary(
                "table.date_add",
                &batch,
                json!({"column": "name", "input_format": "%Y", "amount": 1, "unit": "days", "output_column": "da"}),
                |b| {
                    dates::date_add(
                        b,
                        &cfg(&json!({"column": "name", "input_format": "%Y", "amount": 1, "unit": "days", "output_column": "da"})),
                    )
                },
            );
            check_unary(
                "table.date_diff",
                &batch,
                json!({"start_column": "name", "end_column": "name", "input_format": "%Y", "unit": "days", "output_column": "dd"}),
                |b| {
                    dates::date_diff(
                        b,
                        &cfg(&json!({"start_column": "name", "end_column": "name", "input_format": "%Y", "unit": "days", "output_column": "dd"})),
                    )
                },
            );
            check_unary(
                "table.timezone_convert",
                &batch,
                json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Europe/Rome", "output_column": "tc"}),
                |b| {
                    dates::timezone_convert(
                        b,
                        &cfg(&json!({"column": "name", "input_format": "%Y", "source_timezone": "UTC", "target_timezone": "Europe/Rome", "output_column": "tc"})),
                    )
                },
            );
            check_unary(
                "table.aggregate",
                &batch,
                json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "sum"}, {"column": "id", "function": "count"}]}),
                |b| {
                    aggregation::aggregate(
                        b,
                        &cfg(&json!({"group_by": ["name"], "aggregations": [{"column": "value", "function": "sum"}, {"column": "id", "function": "count"}]})),
                    )
                },
            );
            check_unary(
                "table.rolling_window",
                &batch,
                json!({"column": "value", "function": "sum", "window": 2, "output_column": "rw"}),
                |b| {
                    aggregation::rolling_window(
                        b,
                        &cfg(&json!({"column": "value", "function": "sum", "window": 2, "output_column": "rw"})),
                    )
                },
            );
            check_unary(
                "table.window_function",
                &batch,
                json!({"column": "value", "function": "rank"}),
                |b| aggregation::window_function(b, &cfg(&json!({"column": "value", "function": "rank"}))),
            );
            check_unary(
                "table.coalesce",
                &batch,
                json!({"columns": ["value"], "output_column": "c"}),
                |b| quality::coalesce(b, &cfg(&json!({"columns": ["value"], "output_column": "c"}))),
            );
            check_unary(
                "table.formula",
                &batch,
                json!({"new_column": "f", "formula": "value * 2"}),
                |b| formula::formula(b, &cfg(&json!({"new_column": "f", "formula": "value * 2"}))),
            );
            check_unary(
                "table.formula",
                &batch,
                json!({"new_column": "f", "formula": "name + 'x'"}),
                |b| formula::formula(b, &cfg(&json!({"new_column": "f", "formula": "name + 'x'"}))),
            );
            check_unary(
                "table.expression",
                &batch,
                json!({"output_column": "e", "expression": {"kind": "binary", "op": "add", "left": {"kind": "column", "name": "value"}, "right": {"kind": "literal", "value": 1}}}),
                |b| {
                    expressions::expression(
                        b,
                        &cfg(&json!({"output_column": "e", "expression": {"kind": "binary", "op": "add", "left": {"kind": "column", "name": "value"}, "right": {"kind": "literal", "value": 1}}})),
                    )
                },
            );
            check_unary(
                "table.expression",
                &batch,
                json!({"output_column": "e", "expression": {"kind": "column", "name": "name"}, "output_type": "text"}),
                |b| {
                    expressions::expression(
                        b,
                        &cfg(&json!({"output_column": "e", "expression": {"kind": "column", "name": "name"}, "output_type": "text"})),
                    )
                },
            );
            check_unary(
                "table.flatten_json",
                &batch,
                json!({"column": "name", "output_columns": ["name_a"]}),
                |b| {
                    analysis::flatten_json(
                        b,
                        &cfg(&json!({"column": "name", "output_columns": ["name_a"]})),
                        &limits,
                    )
                },
            );
            check_unary(
                "table.melt",
                &batch,
                json!({"id_columns": ["id"], "value_columns": ["value"]}),
                |b| {
                    reshape::melt(
                        b,
                        &cfg(&json!({"id_columns": ["id"], "value_columns": ["value"]})),
                        &limits,
                    )
                },
            );
            check_unary(
                "table.assert_schema",
                &batch,
                json!({"fields": [{"name": "id", "data_type": "int64", "nullable": false}], "allow_extra": true}),
                |b| {
                    quality::assert_schema(
                        b,
                        &cfg(&json!({"fields": [{"name": "id", "data_type": "int64", "nullable": false}], "allow_extra": true})),
                    )
                },
            );
            check_unary("table.assert_not_null", &batch, json!({"columns": ["id"]}), |b| {
                quality::assert_not_null(b, &cfg(&json!({"columns": ["id"]})))
            });
            check_unary("table.assert_unique", &batch, json!({"columns": ["id"]}), |b| {
                quality::assert_unique(b, &cfg(&json!({"columns": ["id"]})))
            });
            check_unary(
                "table.assert_range",
                &batch,
                json!({"column": "value", "min": 0, "allow_null": true}),
                |b| {
                    quality::assert_range(
                        b,
                        &cfg(&json!({"column": "value", "min": 0, "allow_null": true})),
                    )
                },
            );
            check_unary(
                "table.assert_regex",
                &batch,
                json!({"column": "name", "pattern": "^[ab]$", "allow_null": true}),
                |b| {
                    quality::assert_regex(
                        b,
                        &cfg(&json!({"column": "name", "pattern": "^[ab]$", "allow_null": true})),
                    )
                },
            );
            check_unary("table.assert_cardinality", &batch, json!({"min_rows": 1}), |b| {
                governance::assert_cardinality(b, &cfg(&json!({"min_rows": 1})))
            });
            check_unary("table.assert_metadata", &batch, json!({"expected": {}}), |b| {
                governance::assert_metadata(b, &cfg(&json!({"expected": {}})))
            });

            let nested = nested_batch();
            check_unary_plain("table.explode", &nested, json!({"column": "lst"}), |b| {
                reshape::explode(b, &cfg(&json!({"column": "lst"})), &limits)
            });
            check_unary_plain("table.unnest", &nested, json!({"column": "st"}), |b| {
                reshape::unnest(b, &cfg(&json!({"column": "st"})), &limits)
            });
        }

        #[test]
        #[allow(clippy::too_many_lines)]
        fn binary_ops_match_kernel_schemas() {
            let batch = simple_batch();
            let right = right_batch();
            let limits = Limits::default();

            check_binary(
                "table.join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    joins::join(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
            check_binary(
                "table.join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"], "how": "outer"}),
                |l, r| {
                    joins::join(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"], "how": "outer"})),
                        &limits,
                    )
                },
            );
            check_binary("table.cross_join", &batch, &right, json!({}), |l, r| {
                joins::cross_join(l, r, &cfg(&json!({})), &limits)
            });
            check_binary(
                "table.semi_join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| joins::semi_join(l, r, &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]}))),
            );
            check_binary(
                "table.anti_join",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| joins::anti_join(l, r, &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]}))),
            );
            check_binary(
                "table.asof_join",
                &batch,
                &right,
                json!({"left_on": "id", "right_on": "rid"}),
                |l, r| {
                    joins::asof_join(
                        l,
                        r,
                        &cfg(&json!({"left_on": "id", "right_on": "rid"})),
                        &limits,
                    )
                },
            );
            check_binary("table.concat", &batch, &batch, json!({}), |l, r| {
                joins::concat(l, r, &cfg(&json!({})), &limits)
            });
            check_binary("table.union_distinct", &batch, &batch, json!({}), |l, r| {
                setops::union_distinct(l, r, &cfg(&json!({})), &limits)
            });
            check_binary("table.intersect", &batch, &batch, json!({}), |l, r| {
                setops::intersect(l, r, &cfg(&json!({})))
            });
            check_binary("table.except", &batch, &batch, json!({}), |l, r| {
                setops::except(l, r, &cfg(&json!({})))
            });
            check_binary(
                "table.table_diff",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    reshape::table_diff(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
            check_binary(
                "table.reconcile",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    governance::reconcile(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
            check_binary(
                "table.assert_foreign_key",
                &batch,
                &right,
                json!({"left_keys": ["id"], "right_keys": ["rid"]}),
                |l, r| {
                    governance::assert_foreign_key(
                        l,
                        r,
                        &cfg(&json!({"left_keys": ["id"], "right_keys": ["rid"]})),
                        &limits,
                    )
                },
            );
        }
    }
}
