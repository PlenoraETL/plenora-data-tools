//! Helper condivisi dell'analisi: errori, validazioni di dominio, parsing
//! delle config e utility su contratti e schemi.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
use plenora_core::contract::{
    ContractProperties, DataContract, GeometryColumnContract, GeometryDimensions, GeometryEncoding,
};
use plenora_core::crs::{required_definition, ResolvedCrs};
use plenora_core::{PlenoraError, Result};
use serde_json::Value;

use crate::arrow_adapter::{
    geo_metadata_json_with_encoding, GEO_METADATA_KEY, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION,
};

// ---------------------------------------------------------------------------
// Errori e validazioni di dominio (messaggi coerenti col protocollo legacy).
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn invalid_param(op: &str, name: &'static str, reason: &'static str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("{op}: parametro `{name}` non valido: {reason}"))
}

pub(in crate::analyze) fn parse_config<T: serde::de::DeserializeOwned>(op: &str, config: &Value) -> Result<T> {
    serde_json::from_value(config.clone())
        .map_err(|error| PlenoraError::InvalidPlan(format!("{op}: config non valida: {error}")))
}

pub(in crate::analyze) fn ensure_finite(op: &str, name: &'static str, value: f64) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_param(op, name, "deve essere finito"))
    }
}

pub(in crate::analyze) fn ensure_non_negative(op: &str, name: &'static str, value: f64) -> Result<()> {
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
pub(in crate::analyze) fn output_name<'a>(op: &str, configured: Option<&'a str>, default: &'a str) -> Result<&'a str> {
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
pub(in crate::analyze) fn validate_wkb_hex(op: &str, name: &'static str, hex: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || hex.is_empty() {
        return Err(invalid_param(op, name, "WKB esadecimale non valido"));
    }
    let bytes: std::result::Result<Vec<u8>, _> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
        .collect();
    let bytes = bytes.map_err(|_| invalid_param(op, name, "WKB esadecimale non valido"))?;
    crate::validate_wkb_contract(&bytes)?;
    Ok(bytes)
}

/// Decodifica e valida strutturalmente il WKB hex del secondo operando.
pub(in crate::analyze) fn validate_other_wkb(op: &str, hex: &str) -> Result<()> {
    validate_wkb_hex(op, "other_wkb", hex).map(|_| ())
}

// ---------------------------------------------------------------------------
// Helper su contratti e schemi.
// ---------------------------------------------------------------------------

/// v1: esattamente una colonna geometria attiva per input (D16).
pub(in crate::analyze) fn single_geometry<'a>(op: &str, input: &'a DataContract) -> Result<&'a GeometryColumnContract> {
    if input.geometries.len() != 1 {
        return Err(PlenoraError::Schema(format!(
            "{op}: l'input deve avere esattamente una colonna geometria attiva (v1), trovate {}",
            input.geometries.len()
        )));
    }
    Ok(&input.geometries[0])
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

pub(in crate::analyze) fn rebuild(input: &DataContract, fields: Vec<Field>, properties: ContractProperties) -> Result<DataContract> {
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

/// Merge dei metadati di SCHEMA delle due sorgenti di un'op binaria (R2.4):
/// chiave in una sola sorgente -> copiata; in entrambe con lo stesso valore
/// -> copiata; in entrambe con valori diversi -> errore esplicito che nomina
/// la chiave (mai i valori: errori senza dati). Le chiavi di `right` sono
/// esaminate in ordine lessicografico: l'eventuale errore e' deterministico
/// (ADR-0001), mai dipendente dall'ordine di iterazione della mappa.
pub(in crate::analyze) fn merge_schema_metadata(
    op: &str,
    left: &DataContract,
    right: &DataContract,
) -> Result<HashMap<String, String>> {
    let mut merged = left.schema.metadata().clone();
    let mut right_keys: Vec<&String> = right.schema.metadata().keys().collect();
    right_keys.sort();
    for key in right_keys {
        let value = &right.schema.metadata()[key];
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
/// proprieta' invariati): usato dalle op binarie per applicare il merge R2.4.
pub(in crate::analyze) fn with_schema_metadata(contract: &DataContract, metadata: HashMap<String, String>) -> Result<DataContract> {
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
/// R2.4 identity-preserving: TUTTI i metadati del campo sorgente sono
/// clonati — incluse le chiavi canoniche `plenora.*` gia' presenti in
/// input — perche' il campo geometria sopravvive invariato (stesso nome,
/// stessi byte); l'emissione canonica resta centralizzata a valle
/// (`executor.rs::canonical_output_schema`), qui non si aggiunge nulla.
pub(in crate::analyze) fn geometry_field(input: &DataContract, geometry: &GeometryColumnContract, nullable: bool) -> Result<Field> {
    let field = input
        .schema
        .field_with_name(&geometry.name)
        .map_err(|_| {
            PlenoraError::Schema(format!(
                "colonna geometria `{}` assente dallo schema",
                geometry.name
            ))
        })?;
    Ok(Field::new(geometry.name.clone(), DataType::Binary, nullable)
        .with_metadata(field.metadata().clone()))
}

/// Nuovo campo geometria con metadati di estensione `geoarrow.wkb` +
/// `geo.crs` + `geo.dimensions` (B1.3: la dimensionalita' scritta e' quella
/// del contratto di output, mai un `xy` silenzioso) + `geo.encoding` (B1.4:
/// la chiave e' scritta solo quando il contratto la dichiara — `Some` — e
/// omessa con `None`: fingerprint e retrocompatibilita' invariati).
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

/// Aggiorna il metadato `geo.crs` del campo geometria (solo `reproject`),
/// preservando dimensionalita' ed encoding dichiarati dal contratto (B1.3 /
/// B1.4: il metadato riscritto resta coerente col contratto — un encoding
/// dichiarato non si perde attraversando il kernel).
pub(in crate::analyze) fn set_geometry_crs(fields: &mut [Field], geometry: &GeometryColumnContract, crs: &ResolvedCrs) -> Result<()> {
    for field in fields.iter_mut() {
        if field.name() == &geometry.name {
            let mut metadata = field.metadata().clone();
            metadata.insert(
                GEO_METADATA_KEY.to_owned(),
                geo_metadata_json_with_encoding(
                    crs.definition(),
                    geometry.dimensions,
                    geometry.encoding,
                )?,
            );
            *field = field.clone().with_metadata(metadata);
            return Ok(());
        }
    }
    Err(PlenoraError::Schema(format!(
        "colonna geometria `{}` assente dallo schema",
        geometry.name
    )))
}

/// Rifiuto a compile-plan (B1.3) per i kernel geo che ELABORANO la geometria
/// decodificandola in `geo::Geometry<f64>` (XY): ogni dimensionalita' diversa
/// da `Xy` — Z/M dichiarate oppure `Unknown` (R3.4: mai mappata a Xy) — e'
/// rifiutata qui, in validazione del piano, mai scoperta a meta' esecuzione
/// (il decode fallirebbe a runtime sulla prima cella Z/M). Il trasporto dei
/// byte Z/M resta possibile con le op tabellari, che li propagano invariati.
pub(in crate::analyze) fn require_xy_dimensions(op: &str, geometry: &GeometryColumnContract) -> Result<()> {
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
/// coincide, altrimenti backend (fail-closed senza `proj-backend`).
pub(in crate::analyze) fn resolve_definition(definition: &str, plan_crs: Option<&ResolvedCrs>) -> Result<ResolvedCrs> {
    required_definition(Some(definition), "crs")?;
    if let Some(plan) = plan_crs {
        if plan.definition() == definition {
            return Ok(plan.clone());
        }
    }
    resolve_crs_backend(definition)
}

#[cfg(feature = "proj-backend")]
pub(in crate::analyze) fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    crate::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}

#[cfg(not(feature = "proj-backend"))]
pub(in crate::analyze) fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    plenora_core::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}
