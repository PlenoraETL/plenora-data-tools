//! Helper condivisi dell'analisi: errori, validazioni di dominio, parsing
//! delle config e utility su contratti e schemi.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Metadata, Schema};
use plenora_core::catalog::{CrsRequirement, OperationDescriptor};
use plenora_core::contract::{
    ContractProperties, ContractProperty, DataContract, GeometryColumnContract, GeometryDimensions,
    GeometryEncoding, GeometryTypesProperty, PropertyConfidence, PropertyScope,
};
use plenora_core::crs::{required_definition, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use crate::arrow_adapter::{
    geo_metadata_json_with_encoding, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION,
    GEO_METADATA_KEY,
};

// ---------------------------------------------------------------------------
// Errori e validazioni di dominio.
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn invalid_param(
    op: &str,
    name: &'static str,
    reason: &'static str,
) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("{op}: parametro `{name}` non valido: {reason}"))
}

/// Il parametro non e' decodificabile — **oppure** il difetto e' nostro.
///
/// La porta WKB rende `Internal` quando la validazione OGC non conclude:
/// quell'errore passa intatto, perche' «parametro non valido» accuserebbe un
/// WKB che nessuno ha dimostrato sbagliato e ne cambierebbe la categoria. Si
/// legge la categoria e non la variante, cosi' un errore avvolto (`Tagged`)
/// non sfugge.
pub(in crate::analyze) fn parametro_non_decodificabile(
    op: &str,
    name: &'static str,
    reason: &'static str,
    error: &PlenoraError,
) -> PlenoraError {
    if error.category() == plenora_core::ErrorCategory::Internal {
        return PlenoraError::Internal(format!("{op}: parametro `{name}`: {error}"));
    }
    invalid_param(op, name, reason)
}

pub(in crate::analyze) fn parse_config<T: serde::de::DeserializeOwned>(
    op: &str,
    config: &Value,
) -> Result<T> {
    serde_json::from_value(config.clone())
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: config non valida: {error}")))
}

/// Il `crs_requirement` dichiarato dal catalogo per l'op; la sua assenza e'
/// un piano non valido, mai un requisito implicito. `op` e' il nome che il
/// messaggio riporta.
pub(in crate::analyze) fn crs_requirement(
    op: &str,
    descriptor: &OperationDescriptor,
) -> Result<CrsRequirement> {
    descriptor.crs_requirement.ok_or_else(|| {
        PlenoraError::InvalidPlan(format!("{op}: crs_requirement assente nel catalogo"))
    })
}

pub(in crate::analyze) fn ensure_finite(op: &str, name: &'static str, value: f64) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_param(op, name, "deve essere finito"))
    }
}

pub(in crate::analyze) fn ensure_non_negative(
    op: &str,
    name: &'static str,
    value: f64,
) -> Result<()> {
    ensure_finite(op, name, value)?;
    if value < 0.0 {
        return Err(invalid_param(op, name, "deve essere non negativo"));
    }
    Ok(())
}

pub(in crate::analyze) fn ensure_positive(op: &str, name: &'static str, value: f64) -> Result<()> {
    ensure_finite(op, name, value)?;
    if value <= 0.0 {
        return Err(invalid_param(op, name, "deve essere maggiore di zero"));
    }
    Ok(())
}

pub(in crate::analyze) fn ensure_ratio(op: &str, name: &'static str, value: f64) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid_param(
            op,
            name,
            "deve essere finito e compreso tra zero e uno",
        ));
    }
    Ok(())
}

pub(in crate::analyze) fn ensure_name(name: &str) -> bool {
    !name.trim().is_empty()
}

/// Nome della colonna aggiunta: `output_column` da config o default
/// documentato; vuoto rifiutato.
pub(in crate::analyze) fn output_name<'a>(
    op: &str,
    configured: Option<&'a str>,
    default: &'a str,
) -> Result<&'a str> {
    let name = configured.unwrap_or(default);
    if ensure_name(name) {
        Ok(name)
    } else {
        Err(invalid_param(op, "output_column", "non deve essere vuoto"))
    }
}

/// Id breve dell'operazione (senza namespace `geo.`): default dei nomi di
/// colonna per misure e predicati.
pub(in crate::analyze) fn short_id(op: &str) -> &str {
    op.strip_prefix("geo.").unwrap_or(op)
}

/// Decodifica e valida strutturalmente un WKB esadecimale da config.
///
/// Si lavora sui byte e non su `&str`: una lunghezza in byte pari non
/// garantisce confini di carattere (`"a\u{e9}b"`), e affettare una stringa
/// fuori da un confine e' un panic che i lint anti-panico non vedono. Ogni byte non
/// ASCII o non esadecimale e' un errore esplicito.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la stringa e' vuota, di lunghezza dispari, o contiene un
/// byte che non e' una cifra esadecimale ASCII; gli errori di
/// [`crate::validate_wkb_contract`] sul contenuto decodificato.
pub(in crate::analyze) fn validate_wkb_hex(
    op: &str,
    name: &'static str,
    hex: &str,
) -> Result<Vec<u8>> {
    let bytes = crate::wkb_hex_to_bytes(hex)
        .ok_or_else(|| invalid_param(op, name, "WKB esadecimale non valido"))?;
    crate::validate_wkb_contract(&bytes)?;
    Ok(bytes)
}

/// Decodifica e valida strutturalmente il WKB hex del secondo operando.
pub(in crate::analyze) fn validate_other_wkb(op: &str, hex: &str) -> Result<()> {
    validate_wkb_hex(op, "other_wkb", hex).map(|_| ())
}

/// Validita' OGC e dominio di validita' del CRS dell'input sulle coordinate
/// del secondo operando `other_wkb` (convenzione: il secondo operando e' nel
/// CRS dell'input).
///
/// Rifa' la validazione strutturale di [`validate_other_wkb`] per avere i
/// byte: il costo e' per piano, non per riga. La decodifica e' quella
/// completa ([`crate::geometry_from_wkb`], validazione OGC compresa), come
/// per `point_wkb` e `reference_wkb`: ogni kernel che riceve `other_wkb`
/// (distanze, predicati, lama di `split`) rifiuterebbe una geometria non
/// valida alla prima riga, e il rifiuto sta qui, in validazione.
pub(in crate::analyze) fn validate_other_wkb_domain(
    op: &str,
    hex: &str,
    input: &DataContract,
) -> Result<geo::Geometry<f64>> {
    let crs = input_crs(op, input)?;
    let bytes = validate_wkb_hex(op, "other_wkb", hex)?;
    let geometry = crate::geometry_from_wkb(&bytes).map_err(|error| {
        parametro_non_decodificabile(op, "other_wkb", "WKB non decodificabile", &error)
    })?;
    validate_config_geometry_domain(op, "other_wkb", &geometry, crs)?;
    Ok(geometry)
}

/// Il CRS risolto della colonna geometria dell'input, per le geometrie che
/// arrivano dalla config con lo stesso CRS.
pub(in crate::analyze) fn input_crs<'a>(
    op: &str,
    input: &'a DataContract,
) -> Result<&'a ResolvedCrs> {
    super::dispatch::require_resolved_crs(op, single_geometry(op, input)?)
}

/// Dominio di validita' del CRS sulle coordinate di una geometria da config.
///
/// L'errore nomina operazione e parametro, mai la coordinata.
pub(in crate::analyze) fn validate_config_geometry_domain(
    op: &str,
    name: &'static str,
    geometry: &geo::Geometry<f64>,
    crs: &ResolvedCrs,
) -> Result<()> {
    crate::crs::validate_geometry_domain(geometry, crs)
        .map_err(|error| PlenoraError::Crs(format!("{op}: parametro `{name}`: {error}")))
}

// ---------------------------------------------------------------------------
// Helper su contratti e schemi.
// ---------------------------------------------------------------------------

/// Esattamente una colonna geometria attiva per input: il secondo operando
/// di un'operazione binaria arriva da un secondo input o dalla config.
pub(in crate::analyze) fn single_geometry<'a>(
    op: &str,
    input: &'a DataContract,
) -> Result<&'a GeometryColumnContract> {
    if input.geometries.len() != 1 {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input deve avere esattamente una colonna geometria attiva (v1), trovate {}",
            input.geometries.len()
        )));
    }
    Ok(&input.geometries[0])
}

/// Identificazione della colonna geometria sul campo dello schema.
///
/// Estensione `geoarrow.wkb` oppure sole chiavi canoniche
/// (`plenora.geometry.*`), lo stesso criterio del trasporto
/// ([`crate::arrow_adapter::field_declares_wkb_geometry`]). Una colonna che
/// il trasporto non saprebbe identificare si ferma in analisi, mai a
/// meta' esecuzione.
pub(in crate::analyze) fn require_identifiable_geometry(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
) -> Result<()> {
    let field = input.schema.field_with_name(&geometry.name).map_err(|_| {
        PlenoraError::Schema(format!(
            "{op}: colonna geometria `{}` assente dallo schema",
            geometry.name
        ))
    })?;
    if crate::arrow_adapter::field_declares_wkb_geometry(field) {
        return Ok(());
    }
    Err(PlenoraError::Schema(format!(
        "{op}: colonna geometria `{}` non identificabile come geometria WKB: mancano \
         sia l'estensione `geoarrow.wkb` sia le chiavi canoniche `plenora.geometry.*`",
        geometry.name
    )))
}

/// Contratto di output di un'operazione che RISCRIVE i tipi geometrici
/// della colonna.
///
/// La proprieta' `types` dichiara i tipi dell'OUTPUT; le chiavi canoniche
/// `types`/`types_declaration` ereditate sono rimosse e
/// `plenora_core::contract::arrow_schema::arrow_schema_from_contract` le
/// ri-emette dal contratto, senza conflitto con la chiave ereditata. Il resto e' preservato (stesso `FieldId`, in place).
pub(in crate::analyze) fn with_geometry_types(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    types: GeometryTypesProperty,
) -> Result<DataContract> {
    with_geometry_types_property(
        input,
        geometry,
        ContractProperty::new(PropertyConfidence::Declared(types), PropertyScope::Schema),
    )
}

/// Come [`with_geometry_types`], con la proprieta' intera: anche
/// `Unknown`, per un'operazione che toglie la dichiarazione ereditata senza
/// poterne dare una.
pub(in crate::analyze) fn with_geometry_types_property(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    types: ContractProperty<GeometryTypesProperty>,
) -> Result<DataContract> {
    let fields: Vec<Field> = input
        .schema
        .fields()
        .iter()
        .map(|field| {
            if field.name() == &geometry.name {
                let mut metadata = field.metadata().clone();
                crate::arrow_adapter::strip_rewritten_types_declarations(&mut metadata);
                field.as_ref().clone().with_metadata(metadata)
            } else {
                field.as_ref().clone()
            }
        })
        .collect();
    let mut geometries = input.geometries.clone();
    let Some(target) = geometries
        .iter_mut()
        .find(|candidate| candidate.field_id == geometry.field_id)
    else {
        return Err(PlenoraError::Internal(format!(
            "colonna geometria `{}` assente dal contratto",
            geometry.name
        )));
    };
    target.types = types;
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        geometries,
        input.active_geometry,
        input.properties.clone(),
    )
}

pub(in crate::analyze) fn output_fields(input: &DataContract) -> Vec<Field> {
    input
        .schema
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect()
}

pub(in crate::analyze) fn ensure_name_free(op: &str, fields: &[Field], name: &str) -> Result<()> {
    if fields.iter().any(|field| field.name() == name) {
        return Err(PlenoraError::Schema(format!(
            "{op}: la colonna di output `{name}` esiste gia' nello schema"
        )));
    }
    Ok(())
}

pub(in crate::analyze) fn rebuild(
    input: &DataContract,
    fields: Vec<Field>,
    properties: ContractProperties,
) -> Result<DataContract> {
    DataContract::new(
        Arc::new(Schema::new_with_metadata(
            fields,
            input.schema.metadata().clone(),
        )),
        input.geometries.clone(),
        input.active_geometry,
        properties,
    )
}

/// Merge dei metadati di SCHEMA delle due sorgenti di un'op binaria:
/// chiave in una sola sorgente -> copiata; in entrambe con lo stesso valore
/// -> copiata; in entrambe con valori diversi -> errore esplicito che nomina
/// la chiave (mai i valori: errori senza dati). Le chiavi di `right` sono
/// esaminate in ordine lessicografico: l'eventuale errore e' deterministico,
/// mai dipendente dall'ordine di iterazione della mappa.
pub(in crate::analyze) fn merge_schema_metadata(
    op: &str,
    left: &DataContract,
    right: &DataContract,
) -> Result<Metadata> {
    let mut merged = left.schema.metadata().clone();
    // `Metadata` itera in ordine di chiave (`BTreeMap`).
    for (key, value) in right.schema.metadata() {
        match merged.get(key) {
            None => {
                merged.insert(key.clone(), value.clone());
            }
            Some(existing) if existing == value => {}
            Some(_) => {
                return Err(PlenoraError::InvalidPlan(format!(
                    "{op}: metadato di schema `{key}` in conflitto fra le due sorgenti"
                )));
            }
        }
    }
    Ok(merged)
}

/// Sostituisce i metadati di SCHEMA del contratto (campi, geometrie e
/// proprieta' invariati): usato dalle op binarie per applicare il merge
/// di [`merge_schema_metadata`].
pub(in crate::analyze) fn with_schema_metadata(
    contract: &DataContract,
    metadata: Metadata,
) -> Result<DataContract> {
    DataContract::new(
        Arc::new(Schema::new_with_metadata(output_fields(contract), metadata)),
        contract.geometries.clone(),
        contract.active_geometry,
        contract.properties.clone(),
    )
}

/// Copia del campo geometria con nullability aggiornata (per gli output a
/// sole geometrie, dove l'aggregazione puo' produrre null).
///
/// Lineage identity-preserving: si clonano TUTTI i metadati del campo, chiavi
/// `plenora.*` comprese, perche' il campo sopravvive invariato; l'emissione
/// canonica resta in
/// `plenora_core::contract::arrow_schema::arrow_schema_from_contract`.
pub(in crate::analyze) fn geometry_field(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    nullable: bool,
) -> Result<Field> {
    let field = input.schema.field_with_name(&geometry.name).map_err(|_| {
        PlenoraError::Schema(format!(
            "colonna geometria `{}` assente dallo schema",
            geometry.name
        ))
    })?;
    Ok(
        Field::new(geometry.name.clone(), DataType::Binary, nullable)
            .with_metadata(field.metadata().clone()),
    )
}

/// Nuovo campo geometria con metadati di estensione `geoarrow.wkb` +
/// `geo.crs` + `geo.dimensions` (la dimensionalita' scritta e' quella
/// del contratto di output, mai un `xy` silenzioso) + `geo.encoding` (
/// la chiave e' scritta solo quando il contratto la dichiara — `Some` — e
/// omessa con `None`: metadati identici a quelli senza encoding).
pub(in crate::analyze) fn new_geometry_field(
    name: &str,
    crs: &ResolvedCrs,
    dimensions: GeometryDimensions,
    encoding: Option<GeometryEncoding>,
    nullable: bool,
) -> Result<Field> {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    metadata.insert(
        GEO_METADATA_KEY.to_owned(),
        geo_metadata_json_with_encoding(crs.definition(), dimensions, encoding)?,
    );
    Ok(Field::new(name, DataType::Binary, nullable).with_metadata(metadata))
}

/// Rifiuto a compile-plan per i kernel geo che ELABORANO la geometria
/// decodificandola in `geo::Geometry<f64>` (XY): ogni dimensionalita' diversa
/// da `Xy` — Z/M dichiarate oppure `Unknown` (mai mappata a Xy) — e'
/// rifiutata qui, in validazione del piano, mai scoperta a meta' esecuzione
/// (il decode fallirebbe a runtime sulla prima cella Z/M). Il trasporto dei
/// byte Z/M resta possibile con le op tabellari, che li propagano invariati.
pub(in crate::analyze) fn require_xy_dimensions(
    op: &str,
    geometry: &GeometryColumnContract,
) -> Result<()> {
    if geometry.dimensions == GeometryDimensions::Xy {
        return Ok(());
    }
    Err(PlenoraError::Unsupported(format!(
        "{op}: dimensionalita' geometria `{}` non supportata: il kernel decodifica \
         in XY e accetta solo `xy` (le op tabellari propagano i byte invariati)",
        geometry.dimensions
    )))
}

/// Risoluzione CRS in analisi: riuso del CRS di piano se la definizione
/// coincide, altrimenti la tabella dei CRS integrati (fail-closed su tutto
/// il resto: non c'e' backend PROJ).
pub(in crate::analyze) fn resolve_definition(
    definition: &str,
    plan_crs: Option<&ResolvedCrs>,
) -> Result<ResolvedCrs> {
    required_definition(Some(definition), "crs")?;
    if let Some(plan) = plan_crs {
        if plan.definition() == definition {
            return Ok(plan.clone());
        }
    }
    resolve_crs_backend(definition)
}

pub(in crate::analyze) fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    plenora_core::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}
