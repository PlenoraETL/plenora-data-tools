//! Helper condivisi dell'analisi: errori, validazioni di dominio, parsing
//! delle config e utility su contratti e schemi.

use std::collections::HashMap;
use std::sync::Arc;

use plenora_core::arrow::{DataType, Field, Schema};
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
// Errori e validazioni di dominio (messaggi coerenti col protocollo legacy).
// ---------------------------------------------------------------------------

pub(in crate::analyze) fn invalid_param(
    op: &str,
    name: &'static str,
    reason: &'static str,
) -> PlenoraError {
    PlenoraError::InvalidPlan(format!("{op}: parametro `{name}` non valido: {reason}"))
}

pub(in crate::analyze) fn parse_config<T: serde::de::DeserializeOwned>(
    op: &str,
    config: &Value,
) -> Result<T> {
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
/// # Perche' sui byte e non su `&str`
///
/// La versione precedente affettava la stringa per indici di byte
/// (`&hex[index..index + 2]`) dopo aver controllato che la LUNGHEZZA IN BYTE
/// fosse pari. Le due cose non si implicano: `"a\u{e9}b"` e' lungo quattro
/// byte — pari — ma l'indice 2 cade in mezzo alla codifica UTF-8 di `\u{e9}`,
/// e affettare fuori da un confine di carattere e' un **panic**, non un
/// errore. L'input arriva dalla configurazione di un piano, quindi da fuori.
///
/// Trovato dalla campagna fuzz notturna (`analyze_geo`, artefatto
/// `crash-fd1eba39798feba74d4fc8837358f35c82a4a34a`). Il lint anti-panic R6
/// non lo copriva: non c'e' nessun `unwrap`/`expect`/`panic!`, il panic e'
/// dentro l'indicizzazione.
///
/// Ora ogni byte e' esaminato per se': non ASCII o non esadecimale e' un
/// errore esplicito, mai un panic.
///
/// # Errors
///
/// `InvalidParam` se la stringa e' vuota, di lunghezza dispari, o contiene un
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

// ---------------------------------------------------------------------------
// Helper su contratti e schemi.
// ---------------------------------------------------------------------------

/// v1: esattamente una colonna geometria attiva per input (D16).
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

/// Identificazione della colonna geometria sul campo dello schema
/// (ADR-0009, decisione 8).
///
/// Estensione `geoarrow.wkb` OPPURE sole chiavi canoniche
/// (`plenora.geometry.*`), lo stesso criterio del trasporto
/// ([`crate::arrow_adapter::field_declares_wkb_geometry`]). Il rifiuto vive
/// QUI, in analisi del piano — il modello e' il rifiuto dimensionale B1.3 di
/// [`require_xy_dimensions`]: una colonna che il trasporto non saprebbe
/// identificare e' fermata a compile-plan, mai scoperta a meta' esecuzione
/// (ADR-0008).
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
/// della colonna (ADR-0009, decisione 8).
///
/// La proprieta' `types` dichiara i tipi dell'OUTPUT e le chiavi canoniche
/// `types`/`types_declaration` ereditate dal campo di input sono rimosse —
/// l'emissione (`executor.rs::canonical_output_schema`) le ri-emette dal
/// contratto, senza conflitto R2.6 con la lineage. Tutto il resto e'
/// preservato (stessi campi, stesso `FieldId`: trasformazione in place).
pub(in crate::analyze) fn with_geometry_types(
    input: &DataContract,
    geometry: &GeometryColumnContract,
    types: GeometryTypesProperty,
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
    target.types =
        ContractProperty::new(PropertyConfidence::Declared(types), PropertyScope::Schema);
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
pub(in crate::analyze) fn with_schema_metadata(
    contract: &DataContract,
    metadata: HashMap<String, String>,
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
/// R2.4 identity-preserving: TUTTI i metadati del campo sorgente sono
/// clonati — incluse le chiavi canoniche `plenora.*` gia' presenti in
/// input — perche' il campo geometria sopravvive invariato (stesso nome,
/// stessi byte); l'emissione canonica resta centralizzata a valle
/// (`executor.rs::canonical_output_schema`), qui non si aggiunge nulla.
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
pub(in crate::analyze) fn set_geometry_crs(
    fields: &mut [Field],
    geometry: &GeometryColumnContract,
    crs: &ResolvedCrs,
) -> Result<()> {
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

/// Rifiuto a compile-plan per `geo.reproject`: le coordinate della colonna
/// devono gia' essere nell'ordine GIS normalizzato del CRS SORGENTE
/// (x=longitudine/easting, y=latitudine/northing).
///
/// E' l'invariante che tutto il centro assume — la stessa dichiarata da
/// [`plenora_core::crs::validate_geometry_domain`], che pero' puo' solo
/// verificare i domini e non l'ordine (una coppia (lat, lon) italiana cade
/// dentro il dominio lon/lat). L'unico segnale disponibile e' la chiave
/// canonica `plenora.geometry.axis_order` dichiarata dal produttore, letta
/// qui fail-closed (R5.1) e finora ignorata. Con una dichiarazione
/// divergente `geo.reproject` sbaglierebbe in due modi, entrambi silenziosi:
///
/// - source == target: il backend non costruisce alcuna pipeline e
///   restituisce i byte di input invariati
///   ([`crate::proj_backend::Reprojector::new`]), mentre l'analisi
///   riscrive `axis_order` con l'ordine GIS normalizzato — il metadato
///   contraddirebbe le coordinate che descrive;
/// - source != target: la pipeline PROJ e' normalizzata per la
///   visualizzazione GIS e legge x=longitudine/easting — coordinate
///   dichiarate al contrario verrebbero trasformate come se non lo fossero,
///   producendo un risultato sbagliato invece di un errore.
///
/// La chiave assente resta accettata per il percorso legacy. `unknown`
/// esplicito e' invece onesto per il trasporto ma non dimostra l'ordine fisico
/// necessario a trasformare le coordinate: anche quello fallisce chiuso.
/// Riordinare le coordinate non e' compito di questa op: sarebbe una
/// trasformazione dei dati che nessuno ha chiesto.
pub(in crate::analyze) fn require_normalized_axis_order(
    op: &str,
    input: &DataContract,
    geometry: &GeometryColumnContract,
    source: &ResolvedCrs,
) -> Result<()> {
    let field = input.schema.field_with_name(&geometry.name).map_err(|_| {
        PlenoraError::Schema(format!(
            "{op}: colonna geometria `{}` assente dallo schema",
            geometry.name
        ))
    })?;
    let Some(declared) = crate::arrow_adapter::canonical_geometry_axis_order(field)? else {
        return Ok(());
    };
    let normalized = source.normalized_gis_axis_order();
    if declared == normalized {
        return Ok(());
    }
    Err(PlenoraError::Crs(format!(
        "{op}: colonna geometria `{}`: `axis_order` dichiarato `{declared}`, ma il kernel \
         legge e riemette le coordinate nell'ordine GIS normalizzato `{normalized}` del CRS \
         sorgente — riproiettare qui produrrebbe coordinate e metadato in contraddizione, \
         non un errore. L'ordine va normalizzato a monte, mai in silenzio qui",
        geometry.name
    )))
}

/// Risoluzione CRS in analisi: riuso del CRS di piano se la definizione
/// coincide, altrimenti backend (fail-closed senza `proj-backend`).
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

#[cfg(feature = "proj-backend")]
pub(in crate::analyze) fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    crate::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}

#[cfg(not(feature = "proj-backend"))]
pub(in crate::analyze) fn resolve_crs_backend(definition: &str) -> Result<ResolvedCrs> {
    plenora_core::crs::resolve_crs(definition, "crs").map_err(PlenoraError::from)
}

#[cfg(test)]
mod tests {
    use super::validate_wkb_hex;

    /// Il percorso completo dell'analizzatore, non solo la decodifica: e' da
    /// qui che il fuzzer `analyze_geo` arrivava al panic.
    #[test]
    fn il_wkb_di_config_ostile_e_un_errore_di_piano() {
        for ostile in [
            "a\u{e9}b",
            "\u{e9}\u{e9}",
            "\u{1F642}",
            "0\u{e9}0",
            "zz",
            "abc",
        ] {
            let errore = validate_wkb_hex("geo.within", "other_wkb", ostile)
                .expect_err("input ostile rifiutato, mai un panic");
            assert!(
                errore.to_string().contains("WKB esadecimale non valido"),
                "{ostile:?}: {errore}"
            );
        }
    }
}
