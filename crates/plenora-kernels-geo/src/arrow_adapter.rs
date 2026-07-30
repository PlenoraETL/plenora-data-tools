//! Adapter Arrow per il canone GeoArrow-WKB (rappresentazione).
//!
//! Port Fase 1 da `arrow_transport.rs` di plenora-geo-tools-arrow, limitato
//! alle parti di rappresentazione: metadati di estensione `GeoArrow`
//! (`ARROW:extension:name` = `geoarrow.wkb`), metadato `geo` JSON con le
//! chiavi `crs`, (dalla milestone B1.1, ICD §3.3) `dimensions` e (dalla
//! milestone B1.4) `encoding` — scritta solo quando il contratto la
//! dichiara, mai come default — decode/encode delle celle WKB con limiti
//! per cella e helper
//! sui `RecordBatch`. L'envelope `PLNGEO3`, i checksum, la CLI e gli schemi
//! `TransformArrowSchema`/`PairArrowSchema` non fanno parte di questo modulo
//! (andranno in `plenora-engine`).
//!
//! Unificazione B1.1: questo modulo e' la casa unica dei metadati `GeoArrow`;
//! il trasporto Arrow v3 di `plenora-engine::geo_transport` delega qui
//! (stesso JSON in uscita byte-per-byte).
//!
//! Le geometrie viaggiano in una colonna `Binary`; ogni cella non-null e'
//! validata dal validatore WKB del kernel e i null sono preservati.
//!
//! Milestone B (contratti trasversali v2.0-rc10 §2, proposta in attesa di
//! ratifica; emissione con deroga registrata §15.4/DER-ICD-002 — vedi
//! `docs/deroghe.md` DER-002): protocollo delle chiavi canoniche
//! `plenora.geometry.*` e `plenora.contract.version` (R2.1/R2.2: namespace
//! dedicato, una chiave per nozione, MAI un blob unico). R4.6.3 (rc9/rc10):
//! l'emissione porta anche gli stati `crs_resolution = missing` (nessuna
//! chiave CRS, coerenza R2.2) e `declared_unresolved` (dichiarazioni
//! originali ri-emesse invariate, R4.6.4) — ADR-0009 decisione 7. `plenora.field_id`
//! e' solo letta (R2.2 opzionale; non si emette il `FieldId` di grafo, che
//! non ha significato fuori dal processo — ADR-0009 decisione 3). Questo
//! modulo
//! fornisce emissione da [`GeometryColumnContract`], lettura fail-closed per
//! chiave (R5.1: valore non canonico → errore esplicito, mai ignorato o
//! corretto), coerenza fra chiavi canoniche e metadato legacy `geo` (R2.6:
//! divergenza → il componente fallisce, non sceglie) e completamento per
//! precedenza canonica > legacy > standard esterno (R2.7: completamento, mai
//! arbitrato). Il wiring nei siti di produzione e' la milestone successiva.
//!
//! Errori: le condizioni `ArrowTransportError` del sorgente sono mappate su
//! [`PlenoraError`] preservando i messaggi (colonne/schema → `Schema`, CRS →
//! `Crs`, limiti cella → `Contract`, serializzazione JSON metadati → `Json`).

use std::collections::HashMap;

use geo::Geometry;
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{Array, BinaryArray};
use plenora_core::arrow::{DataType, Field, RecordBatch, Schema};
use plenora_core::contract::{
    AxisOrder, ContractCrs, CrsDefinitionFormat, CrsResolution, FieldId, GeometryColumnContract,
    GeometryDimensions, GeometryEncoding, GeometryPrecision, GeometryTypesProperty,
    SpatialSemantics,
};
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
use plenora_core::PlenoraError;
use rayon::prelude::*;

use crate::geometry_from_wkb;
use crate::memory_estimate::{estimate_geometry_native_bytes, DecodedNativeBytesEstimate};

pub const GEOARROW_EXTENSION_KEY: &str = "ARROW:extension:name";
pub const GEOARROW_WKB_EXTENSION: &str = "geoarrow.wkb";
pub const GEO_METADATA_KEY: &str = "geo";
pub const DEFAULT_GEOMETRY_COLUMN: &str = "geometry";
pub const MAX_CELL_BYTES: u64 = 64 * 1024 * 1024;

/// Chiave canonica del framing binario delle celle (R2.1/R2.2, tabella §2:
/// `wkb` | `ewkb`).
///
/// Obbligatoria per colonne geometriche, ma resta assente se il contratto
/// non la dichiara — R5.2, mai un default al posto dell'assente.
pub const PLENORA_GEOMETRY_ENCODING_KEY: &str = "plenora.geometry.encoding";
/// Chiave canonica della dimensionalita' (R2.1/R2.2: `xy` | `xyz` | `xym` |
/// `xyzm` | `unknown`; obbligatoria, `unknown` e' valore canonico R3.4).
pub const PLENORA_GEOMETRY_DIMENSIONS_KEY: &str = "plenora.geometry.dimensions";
/// Chiave canonica dell'elenco dei tipi (R2.2/R3.4.1: valori unici in ordine
/// §3.1 separati da `,` senza spazi; obbligatoria e non vuota se
/// `types_declaration = exact`).
pub const PLENORA_GEOMETRY_TYPES_KEY: &str = "plenora.geometry.types";
/// Chiave canonica dello stato di dichiarazione dei tipi (R2.2/R3.4.1:
/// `exact` | `mixed` | `unresolved`).
pub const PLENORA_GEOMETRY_TYPES_DECLARATION_KEY: &str = "plenora.geometry.types_declaration";
/// Chiave canonica dello SRID (R2.2: intero decimale senza segno; opzionale,
/// emessa solo se noto).
pub const PLENORA_GEOMETRY_SRID_KEY: &str = "plenora.geometry.srid";
/// Chiave canonica dell'identificatore di autorita' del CRS (R2.2: es.
/// `EPSG:4326`; opzionale).
pub const PLENORA_GEOMETRY_CRS_ID_KEY: &str = "plenora.geometry.crs_id";
/// Chiave canonica dello stato di risoluzione del CRS (R2.2: `resolved` |
/// `declared_unresolved` | `missing`; obbligatoria).
pub const PLENORA_GEOMETRY_CRS_RESOLUTION_KEY: &str = "plenora.geometry.crs_resolution";
/// Chiave canonica della definizione CRS testuale (R2.2: WKT o PROJJSON;
/// opzionale, richiede `crs_definition_format`).
pub const PLENORA_GEOMETRY_CRS_DEFINITION_KEY: &str = "plenora.geometry.crs_definition";
/// Chiave canonica del formato della definizione CRS (R2.2: `wkt` | `wkt2` |
/// `projjson`; obbligatoria se `crs_definition` e' presente).
pub const PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY: &str =
    "plenora.geometry.crs_definition_format";
/// Chiave canonica dell'ordine degli assi (R2.2: `lon_lat` | `lat_lon` |
/// `easting_northing` | `northing_easting` | `other` | `unknown`;
/// obbligatoria se `crs_id` o `crs_definition` e' presente).
pub const PLENORA_GEOMETRY_AXIS_ORDER_KEY: &str = "plenora.geometry.axis_order";
/// Chiave canonica della semantica spaziale (R2.2: `geometry` | `geography`;
/// opzionale).
pub const PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY: &str = "plenora.geometry.spatial_semantics";
/// Chiave canonica della precisione delle coordinate (R2.2: `float64` |
/// `float32` | `native`; opzionale).
pub const PLENORA_GEOMETRY_PRECISION_KEY: &str = "plenora.geometry.precision";
/// Chiave canonica dell'identita' logica stabile della colonna (R2.2:
/// intero decimale senza segno; opzionale).
pub const PLENORA_FIELD_ID_KEY: &str = "plenora.field_id";
/// Chiave di versione del protocollo dei metadati (R2.5: intero decimale,
/// oggi `1`; vive in `Schema::metadata`, MAI nel campo, ed e' obbligatoria
/// se sono presenti chiavi canoniche).
pub const PLENORA_CONTRACT_VERSION_KEY: &str = "plenora.contract.version";
/// Versione corrente del protocollo dei metadati (R2.5): un consumatore che
/// riceve una versione maggiore DEVE fallire in modo esplicito, mai
/// interpretare parzialmente.
pub const PLENORA_CONTRACT_VERSION: u32 = 1;

/// Prefisso del namespace canonico (R2.1): usato per rilevare la presenza di
/// chiavi canoniche nel gate di versione R2.5.
const PLENORA_NAMESPACE_PREFIX: &str = "plenora.";
/// Prefisso del namespace geometrico canonico (`plenora.geometry.*`).
///
/// Un campo che porta almeno una di queste chiavi si dichiara colonna
/// geometrica in forma autosufficiente (tabella §2: encoding e dimensions
/// obbligatorie), anche in assenza dei metadati `GeoArrow` legacy. Resta
/// escluso `plenora.field_id`, che non e' specifico delle geometrie.
pub const PLENORA_GEOMETRY_NAMESPACE_PREFIX: &str = "plenora.geometry.";
/// Lunghezza massima dell'identificatore di autorita' `crs_id` (allineata al
/// lettore di plenora-database-tools, stessa regola di robustezza).
const MAX_CRS_ID_BYTES: usize = 1_024;

/// Coordinate massime per cella: una cella da 64 MiB contiene al piu' 16 byte
/// per coordinata XY.
///
/// Scelta B1.3 (documentata): il bound NON e' reso stride-aware. Con Z/M lo
/// stride reale e' 24/32 byte e il conteggio massimo reale scende, quindi il
/// bound su 16 byte resta permissivo ma sempre sicuro (mai sotto il reale);
/// irrigidirlo richiederebbe la dimensionalita' risolta, che per `Unknown`
/// (R3.4) non esiste. Si mantiene il bound conservativo unico.
pub const MAX_CELL_COORDINATES: u64 = MAX_CELL_BYTES / 16;

fn missing_geometry_column(name: &str) -> PlenoraError {
    PlenoraError::Schema(format!("colonna geometria `{name}` assente"))
}

fn missing_geoarrow_metadata(name: &str) -> PlenoraError {
    PlenoraError::Schema(format!(
        "colonna geometria `{name}` senza metadati estensione geoarrow.wkb"
    ))
}

fn geometry_column_not_binary(name: &str, actual: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::Schema(format!(
        "colonna geometria `{name}` di tipo {actual}, atteso Binary"
    ))
}

fn cell_too_large(bytes: u64) -> PlenoraError {
    PlenoraError::InvalidPlan(format!(
        "cella WKB da {bytes} byte oltre il limite {MAX_CELL_BYTES}"
    ))
}

/// Indice della colonna geometria: deve esistere, essere `Binary` e portare
/// i metadati di estensione `geoarrow.wkb`.
///
/// # Errors
///
/// `PlenoraError::Schema` se la colonna `name` e' assente, non e' di tipo
/// `Binary` o non porta i metadati di estensione `geoarrow.wkb`.
pub fn geometry_column_index(schema: &Schema, name: &str) -> Result<usize, PlenoraError> {
    let (index, field) = schema
        .column_with_name(name)
        .ok_or_else(|| missing_geometry_column(name))?;
    if field.data_type() != &DataType::Binary {
        return Err(geometry_column_not_binary(name, field.data_type()));
    }
    let extension = field.metadata().get(GEOARROW_EXTENSION_KEY);
    if extension.map(String::as_str) != Some(GEOARROW_WKB_EXTENSION) {
        return Err(missing_geoarrow_metadata(name));
    }
    Ok(index)
}

/// Metadato `GeoArrow` `geo` con la chiave `crs`: PROJJSON se la definizione e'
/// gia' un oggetto JSON, altrimenti la forma authority:code come stringa.
///
/// Casa unica del formato (unificazione B1.1): anche il trasporto Arrow v3 di
/// `plenora-engine` delega qui, quindi il JSON in uscita e' identico
/// byte-per-byte nei due percorsi.
///
/// # Errors
///
/// `PlenoraError::Crs` se `crs` e' vuota (o solo spazi) o supera
/// [`MAX_CRS_DEFINITION_BYTES`]; `PlenoraError::DataMapping` se la serializzazione
/// del metadato fallisce.
pub fn geo_metadata_json(crs: &str) -> Result<String, PlenoraError> {
    let metadata = geo_metadata_map(crs)?;
    serde_json::to_string(&serde_json::Value::Object(metadata)).map_err(PlenoraError::from)
}

/// Come [`geo_metadata_json`], con in piu' la chiave `dimensions` in forma
/// ICD ([`GeometryDimensions::as_str`]).
///
/// La propagazione reale della dimensionalita' e' milestone B1.3: qui la
/// scriviamo solo per dichiararla.
///
/// # Errors
///
/// Come [`geo_metadata_json_with_encoding`].
pub fn geo_metadata_json_with_dimensions(
    crs: &str,
    dimensions: GeometryDimensions,
) -> Result<String, PlenoraError> {
    geo_metadata_json_with_encoding(crs, dimensions, None)
}

/// Come [`geo_metadata_json_with_dimensions`], con in piu' la chiave
/// `encoding` in forma ICD ([`GeometryEncoding::as_str`]) quando il contratto
/// la dichiara (`Some`).
///
/// Con `None` la chiave e' omessa e il JSON e' identico byte-per-byte a
/// [`geo_metadata_json_with_dimensions`] (fingerprint e retrocompatibilita'
/// invariati — B1.4).
///
/// # Errors
///
/// Come [`geo_metadata_json`]: `PlenoraError::Crs` se `crs` e' vuota (o solo
/// spazi) o supera [`MAX_CRS_DEFINITION_BYTES`]; `PlenoraError::DataMapping` se la
/// serializzazione del metadato fallisce.
pub fn geo_metadata_json_with_encoding(
    crs: &str,
    dimensions: GeometryDimensions,
    encoding: Option<GeometryEncoding>,
) -> Result<String, PlenoraError> {
    let mut metadata = geo_metadata_map(crs)?;
    metadata.insert(
        "dimensions".to_owned(),
        serde_json::Value::String(dimensions.as_str().to_owned()),
    );
    if let Some(encoding) = encoding {
        metadata.insert(
            "encoding".to_owned(),
            serde_json::Value::String(encoding.as_str().to_owned()),
        );
    }
    serde_json::to_string(&serde_json::Value::Object(metadata)).map_err(PlenoraError::from)
}

/// Mappa `geo` validata con la sola chiave `crs` (corpo condiviso delle due
/// serializzazioni pubbliche).
fn geo_metadata_map(crs: &str) -> Result<serde_json::Map<String, serde_json::Value>, PlenoraError> {
    if crs.trim().is_empty() {
        return Err(PlenoraError::Crs(
            "crs obbligatorio per il trasporto Arrow v3".to_owned(),
        ));
    }
    if crs.len() > MAX_CRS_DEFINITION_BYTES {
        return Err(PlenoraError::Crs(format!(
            "crs oltre il limite di {MAX_CRS_DEFINITION_BYTES} byte"
        )));
    }
    let crs_value = match serde_json::from_str::<serde_json::Value>(crs) {
        Ok(value @ serde_json::Value::Object(_)) => value,
        _ => serde_json::Value::String(crs.to_owned()),
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert("crs".to_owned(), crs_value);
    Ok(metadata)
}

/// Campo `Binary` di output con metadati `geoarrow.wkb` e `geo.crs` +
/// `geo.dimensions`.
///
/// B1.1: la dimensionalita' scritta e' sempre `Xy` (i costruttori attuali
/// producono WKB 2D); la propagazione della dimensionalita' reale e'
/// milestone B1.3.
///
/// # Errors
///
/// Come [`geometry_output_field_with_encoding`].
pub fn geometry_output_field(name: &str, crs: &str) -> Result<Field, PlenoraError> {
    geometry_output_field_with_dimensions(name, crs, GeometryDimensions::Xy)
}

/// Come [`geometry_output_field`], con la dimensionalita' dichiarata
/// esplicitamente (pronto per la propagazione di B1.3).
///
/// # Errors
///
/// Come [`geometry_output_field_with_encoding`].
pub fn geometry_output_field_with_dimensions(
    name: &str,
    crs: &str,
    dimensions: GeometryDimensions,
) -> Result<Field, PlenoraError> {
    geometry_output_field_with_encoding(name, crs, dimensions, None)
}

/// Come [`geometry_output_field_with_dimensions`], con in piu' la chiave
/// `geo.encoding` quando il contratto la dichiara (`Some`).
///
/// B1.4: un contratto con encoding dichiarato che attraversa un kernel che
/// riscrive il campo (es. `reproject`) conserva la chiave nel metadato
/// riscritto, coerente col contratto. Con `None` la chiave e' omessa e il
/// metadato e' identico byte-per-byte alla forma senza encoding (fingerprint
/// e retrocompatibilita' invariati).
///
/// # Errors
///
/// Come [`geo_metadata_json_with_encoding`] (validazioni `crs` e
/// serializzazione JSON del metadato `geo`).
pub fn geometry_output_field_with_encoding(
    name: &str,
    crs: &str,
    dimensions: GeometryDimensions,
    encoding: Option<GeometryEncoding>,
) -> Result<Field, PlenoraError> {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    metadata.insert(
        GEO_METADATA_KEY.to_owned(),
        geo_metadata_json_with_encoding(crs, dimensions, encoding)?,
    );
    Ok(Field::new(name, DataType::Binary, true).with_metadata(metadata))
}

/// Dimensionalita' dichiarata nel metadato `geo` di un campo geometria.
///
/// Lettura opzionale pronta per B1.3: chiave assente, JSON non valido o
/// valore non riconosciuto → [`GeometryDimensions::Unknown`] (R3.4: MAI un
/// default silenzioso `Xy`). La discovery di B1.3 potra' rendere il valore
/// non riconosciuto un errore esplicito; questa lettura non decide.
#[must_use]
pub fn geometry_dimensions_from_metadata(field: &Field) -> GeometryDimensions {
    geo_metadata_value(field)
        .and_then(|value| {
            value
                .get("dimensions")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .and_then(|dimensions| dimensions.parse().ok())
        .unwrap_or(GeometryDimensions::Unknown)
}

/// Encoding dichiarato nel metadato `geo` di un campo geometria.
///
/// Lettura opzionale pronta per B1.3: chiave assente, JSON non valido o
/// valore non riconosciuto → `None` (R3.4/R3.5: MAI un default silenzioso;
/// R3.5: valori fuori dall'enum chiuso non sono rappresentabili).
#[must_use]
pub fn geometry_encoding_from_metadata(field: &Field) -> Option<GeometryEncoding> {
    geo_metadata_value(field).and_then(|value| {
        value
            .get("encoding")
            .and_then(serde_json::Value::as_str)
            .and_then(|encoding| encoding.parse().ok())
    })
}

/// Variante STRICT di [`geometry_encoding_from_metadata`] per la discovery
/// (B1.3).
///
/// La chiave `encoding` presente ma fuori dall'enum chiuso (R3.5: header
/// `GeoPackage`, TWKB, valori non testuali) e' un framing non rappresentabile
/// e va rifiutato con errore esplicito — mai mappata a un encoding noto o
/// ignorata. Chiave assente o metadato `geo` non valido → `Ok(None)` (la
/// dimensionalita'/il framing non dichiarati restano non risolti, R3.4; il
/// messaggio non riporta il valore, «errori senza dati»).
///
/// # Errors
///
/// `PlenoraError::Unsupported` se la chiave `encoding` e' presente ma non
/// rappresentabile: valore non testuale o fuori dall'enum chiuso R3.5
/// (ammessi solo `wkb` ed `ewkb`).
pub fn geometry_encoding_from_metadata_strict(
    field: &Field,
) -> Result<Option<GeometryEncoding>, PlenoraError> {
    let Some(value) = geo_metadata_value(field) else {
        return Ok(None);
    };
    let Some(raw) = value.get("encoding") else {
        return Ok(None);
    };
    let parsed = raw.as_str().and_then(|text| text.parse().ok());
    parsed.map_or_else(
        || {
            Err(PlenoraError::Unsupported(
                "metadato `geo`: encoding geometria non rappresentabile \
                 (R3.5: ammessi solo `wkb` ed `ewkb`)"
                    .to_owned(),
            ))
        },
        |encoding| Ok(Some(encoding)),
    )
}

/// Il metadato `geo` di un campo come valore JSON, se presente e valido.
fn geo_metadata_value(field: &Field) -> Option<serde_json::Value> {
    let raw = field.metadata().get(GEO_METADATA_KEY)?;
    serde_json::from_str::<serde_json::Value>(raw).ok()
}

// ---------------------------------------------------------------------------
// Milestone B — protocollo delle chiavi canoniche (contratti trasversali
// v2.0-rc10 §2, proposta in attesa di ratifica): emissione da
// `GeometryColumnContract`, lettura fail-closed per chiave (R5.1), coerenza
// canonica-vs-legacy (R2.6) e completamento per precedenza (R2.7).
// ---------------------------------------------------------------------------

/// Dettagli della tabella R2.2 che un [`GeometryColumnContract`] NON
/// modella.
///
/// Chiavi opzionali che il produttore dichiara solo se note (R5.2: assenti
/// restano assenti, mai un default al posto dell'assente).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryMetadataDetails {
    /// Ordine degli assi del CRS; se assente e un CRS e' presente l'emissione
    /// dichiara `unknown` (valore canonico esplicito, la chiave e'
    /// obbligatoria quando un CRS e' presente).
    pub axis_order: Option<AxisOrder>,
    /// SRID noto (emesso come intero decimale senza segno).
    pub srid: Option<u32>,
    /// Semantica spaziale della colonna.
    pub spatial_semantics: Option<SpatialSemantics>,
    /// Precisione delle coordinate.
    pub precision: Option<GeometryPrecision>,
}

/// Metadati di campo canonici R2.2 per una colonna geometrica, costruiti da
/// un [`GeometryColumnContract`] e dai dettagli che il contratto non modella.
///
/// Decisioni di formato (il punto delicato e' la coerenza con il metadato
/// legacy `geo` e con l'emissione di plenora-database-tools):
///
/// - `crs_resolution` riflette lo stato del contratto: `resolved` per un
///   `ResolvedCrs` (risolto per costruzione — lo produce solo una risoluzione
///   contro il database PROJ — quindi il valore e' onesto, non un default),
///   `declared_unresolved` per `ContractCrs::DeclaredUnresolved` (R4.6.3:
///   l'incoerenza si propaga dichiarata, mai risolta in silenzio),
///   `missing` per `ContractCrs::Missing` (R4.6.3/R4.6.4: lo stato mancante
///   si propaga invariato, mai un CRS inventato — R4.4);
/// - con `declared_unresolved` le dichiarazioni ORIGINALI sono ri-emesse
///   invariate (R4.6.4: l'incoerenza non risolta arriva al bordo di
///   scrittura — R2.4, mai una persa, mai una inventata): `crs_id` e/o
///   `crs_definition` con il suo `crs_definition_format` (R4.3), cosi' come
///   le porta il contratto; `srid` non e' ri-emesso dal blocco (non e'
///   modellato dal contratto: resta propagato dalla lineage);
/// - con `missing` NON sono emesse `crs_id`/`crs_definition`/
///   `crs_definition_format`/`axis_order`/`srid` (coerenza R2.2:
///   `crs_resolution = missing` non ammette metadati CRS dichiarati);
/// - la definizione CRS che e' un oggetto JSON (PROJJSON) e' emessa come
///   `crs_definition` + `crs_definition_format = projjson`; ogni altra
///   definizione e' emessa come `crs_id` (identificatore di autorita', es.
///   `EPSG:4326`). E' la stessa distinzione che [`geo_metadata_json`]
///   applica al metadato legacy `geo.crs` (oggetto JSON incorporato vs
///   stringa authority:code), cosi' le due rappresentazioni sono coerenti
///   per costruzione (R2.6); e' anche la forma emessa da
///   plenora-database-tools (`crs_id` = `EPSG:xxxx`). Limite dichiarato: una
///   definizione WKT testuale non e' distinguibile da un identificatore di
///   autorita' senza un hint di formato, che `ResolvedCrs` non porta; nel
///   workspace le definizioni risolte sono oggi authority:code o PROJJSON.
/// - con un CRS risolto o dichiarato non risolto `axis_order` e' sempre
///   emesso (obbligatorio quando un
///   CRS e' presente): senza dettaglio esplicito vale `unknown`, valore
///   canonico dichiarato dalla tabella §2 — la chiave qui e' obbligatoria,
///   non opzionale, e `unknown` non e' un default al posto dell'assente (R5.2
///   riguarda le chiavi opzionali: `srid`, `spatial_semantics`, `precision`,
///   `encoding`, che restano assenti se non note).
/// - `types`/`types_declaration` sono emesse SOLO se il campo `types` porta
///   un valore (confidence `Declared`/`Proven`/`Estimated`); confidence
///   `Unknown` («proprieta' non dichiarata», R3.4.1) non emette nulla: mai
///   inventare una dichiarazione. `types` e' omessa quando l'elenco e' vuoto
///   (`unresolved`, o `mixed` senza elenco), come da forma canonica.
///
/// Le chiavi `GeoArrow` (`ARROW:extension:name`, `geo`) RESTANO emesse dai
/// costruttori esistenti (R2.6 ammette la coesistenza se coerente): questa
/// funzione produce solo il blocco canonico; la fusione nei campi di output
/// e' responsabilita' del chiamante (milestone di wiring), cosi' come
/// l'aggiunta di `plenora.contract.version` nei metadati dello schema
/// ([`canonical_schema_version_metadata`]).
#[must_use]
pub fn canonical_geometry_metadata(
    contract: &GeometryColumnContract,
    details: &GeometryMetadataDetails,
) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    if let Some(encoding) = contract.encoding {
        metadata.insert(
            PLENORA_GEOMETRY_ENCODING_KEY.to_owned(),
            encoding.as_str().to_owned(),
        );
    }
    metadata.insert(
        PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
        contract.dimensions.as_str().to_owned(),
    );
    if let Some(types) = contract.types.value() {
        metadata.insert(
            PLENORA_GEOMETRY_TYPES_DECLARATION_KEY.to_owned(),
            types.declaration().as_str().to_owned(),
        );
        let list = types.to_canonical_list();
        if !list.is_empty() {
            metadata.insert(PLENORA_GEOMETRY_TYPES_KEY.to_owned(), list);
        }
    }
    metadata.insert(
        PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
        contract.crs.resolution().as_str().to_owned(),
    );
    match &contract.crs {
        // `ResolvedByDecision` si emette come `Resolved` (un CRS risolto a
        // tutti gli effetti): la sostituzione delle dichiarazioni della
        // sorgente avviene a monte, nella fusione dello schema di output
        // ([`strip_decided_crs_declarations`]).
        ContractCrs::Resolved(crs) | ContractCrs::ResolvedByDecision(crs) => {
            insert_resolved_crs_keys(&mut metadata, crs.definition(), details);
        }
        ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } => {
            // R4.6.4: le dichiarazioni originali sono ri-emesse invariate —
            // l'incoerenza arriva al bordo di scrittura com'era, mai persa
            // e mai conciliata. `axis_order` e' emesso come per `resolved`
            // (obbligatorio quando una rappresentazione CRS e' presente):
            // il default `unknown` resta l'assenza di una dichiarazione e
            // non sovrascrive la lineage (vedi `canonical_output_schema`).
            if let Some(crs_id) = crs_id {
                metadata.insert(PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), crs_id.clone());
            }
            if let Some(definition) = definition {
                metadata.insert(
                    PLENORA_GEOMETRY_CRS_DEFINITION_KEY.to_owned(),
                    definition.clone(),
                );
                if let Some(format) = definition_format {
                    metadata.insert(
                        PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY.to_owned(),
                        format.as_str().to_owned(),
                    );
                }
            }
            let axis_order = details.axis_order.unwrap_or(AxisOrder::Unknown);
            metadata.insert(
                PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                axis_order.as_str().to_owned(),
            );
        }
        // Con `crs_resolution = missing` nessuna chiave CRS e' emessa (R2.2:
        // `missing` non ammette `crs_id`/`crs_definition`/`srid`/`axis_order`).
        ContractCrs::Missing => {}
    }
    if let Some(semantics) = details.spatial_semantics {
        metadata.insert(
            PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY.to_owned(),
            semantics.as_str().to_owned(),
        );
    }
    if let Some(precision) = details.precision {
        metadata.insert(
            PLENORA_GEOMETRY_PRECISION_KEY.to_owned(),
            precision.as_str().to_owned(),
        );
    }
    // `plenora.field_id` NON e' emesso: la tabella R2.2 lo dichiara
    // opzionale e il `FieldId` del contratto appartiene al namespace del
    // grafo che lo ha assegnato (ADR-0009, decisione 3) — non ha
    // significato fuori dal processo. Una chiave `plenora.field_id`
    // RICEVUTA resta propagata invariata dalla lineage (R2.4), mai
    // sovrascritta dal valore di grafo.
    metadata
}

/// Chiavi CRS di uno stato `resolved` (R2.2): corpo condiviso fra
/// [`canonical_geometry_metadata`] (braccio `Resolved`/`ResolvedByDecision`)
/// e [`canonical_geometry_metadata_for_resolved_definition`] — stessa forma
/// e stessi byte a parita' di definizione e dettagli.
///
/// Una definizione che e' un oggetto JSON (PROJJSON) e' emessa come
/// `crs_definition` + `crs_definition_format = projjson`; ogni altra come
/// `crs_id` — la stessa distinzione che [`geo_metadata_json`] applica al
/// metadato legacy `geo.crs`, cosi' le due rappresentazioni restano coerenti
/// per costruzione (R2.6). `axis_order` e' sempre emesso (obbligatorio
/// quando un CRS e' presente): senza dettaglio esplicito vale `unknown`,
/// valore canonico della tabella §2, non un default al posto dell'assente.
fn insert_resolved_crs_keys(
    metadata: &mut HashMap<String, String>,
    definition: &str,
    details: &GeometryMetadataDetails,
) {
    if matches!(
        serde_json::from_str::<serde_json::Value>(definition),
        Ok(serde_json::Value::Object(_))
    ) {
        metadata.insert(
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY.to_owned(),
            definition.to_owned(),
        );
        metadata.insert(
            PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY.to_owned(),
            CrsDefinitionFormat::Projjson.as_str().to_owned(),
        );
    } else {
        metadata.insert(PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), definition.to_owned());
    }
    let axis_order = details.axis_order.unwrap_or(AxisOrder::Unknown);
    metadata.insert(
        PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
        axis_order.as_str().to_owned(),
    );
    if let Some(srid) = details.srid {
        metadata.insert(PLENORA_GEOMETRY_SRID_KEY.to_owned(), srid.to_string());
    }
}

/// Blocco canonico R2.2 da una definizione CRS gia' risolta al bordo del
/// produttore, senza un [`ResolvedCrs`].
///
/// BLOCK-06 (decisione owner 2026-07-30 — parita' del percorso legacy col
/// v4, DER-002 estesa): il trasporto legacy `geo_transport` valida il CRS
/// al livello comandi (risoluzione PROJ obbligatoria in `publish.rs`) e
/// trasporta la sola definizione; un `ResolvedCrs` richiederebbe una
/// risoluzione che il trasporto non esegue.
///
/// Lo stato emesso e' `resolved` — il CRS dichiarato nell'operazione e'
/// stato risolto al bordo comandi, ed e' lo stesso che il metadato legacy
/// `geo.crs` dichiara (coerenza R2.6 per costruzione). La forma e' quella
/// del braccio `Resolved` di [`canonical_geometry_metadata`] (corpo
/// condiviso [`insert_resolved_crs_keys`]: byte identici a parita' di
/// definizione e dettagli). `types`/`types_declaration` NON sono emesse: il
/// trasporto legacy non dichiara i tipi e inventarli e' vietato (R3.4.1);
/// `encoding` e' emessa solo se il chiamante la dichiara (R5.2).
#[must_use]
pub fn canonical_geometry_metadata_for_resolved_definition(
    definition: &str,
    dimensions: GeometryDimensions,
    encoding: Option<GeometryEncoding>,
    details: &GeometryMetadataDetails,
) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    if let Some(encoding) = encoding {
        metadata.insert(
            PLENORA_GEOMETRY_ENCODING_KEY.to_owned(),
            encoding.as_str().to_owned(),
        );
    }
    metadata.insert(
        PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(),
        dimensions.as_str().to_owned(),
    );
    metadata.insert(
        PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
        CrsResolution::Resolved.as_str().to_owned(),
    );
    insert_resolved_crs_keys(&mut metadata, definition, details);
    if let Some(semantics) = details.spatial_semantics {
        metadata.insert(
            PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY.to_owned(),
            semantics.as_str().to_owned(),
        );
    }
    if let Some(precision) = details.precision {
        metadata.insert(
            PLENORA_GEOMETRY_PRECISION_KEY.to_owned(),
            precision.as_str().to_owned(),
        );
    }
    metadata
}

/// Metadati di schema con la versione del protocollo (R2.5:
/// `plenora.contract.version` vive in `Schema::metadata`, non nel campo, ed
/// e' obbligatoria se lo schema porta chiavi canoniche).
#[must_use]
pub fn canonical_schema_version_metadata() -> HashMap<String, String> {
    HashMap::from([(
        PLENORA_CONTRACT_VERSION_KEY.to_owned(),
        PLENORA_CONTRACT_VERSION.to_string(),
    )])
}

/// Chiavi canoniche che una decisione CRS del piano (R4.6.3) sostituisce.
///
/// La decisione rimpiazza le dichiarazioni in conflitto, che non devono
/// sopravvivere accanto al CRS deciso (una `crs_id`/`srid`/`axis_order`
/// della sorgente descriverebbe il CRS deciso con le dichiarazioni che il
/// piano ha esplicitamente superato — e un consumatore a valle leggerebbe
/// di nuovo il conflitto).
pub const CRS_KEYS_REPLACED_BY_DECISION: [&str; 6] = [
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
    PLENORA_GEOMETRY_CRS_ID_KEY,
    PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
    PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
    PLENORA_GEOMETRY_AXIS_ORDER_KEY,
    PLENORA_GEOMETRY_SRID_KEY,
];

/// Rimuove le dichiarazioni CRS dai metadati di un campo geometria.
///
/// Toglie le chiavi canoniche di [`CRS_KEYS_REPLACED_BY_DECISION`] e il
/// membro `crs` del metadato legacy `geo` (se il metadato resta vuoto,
/// rimosso): usato all'emissione quando una decisione esplicita del piano
/// ([`ContractCrs::ResolvedByDecision`], R4.6.3) sostituisce le
/// dichiarazioni della sorgente — il blocco canonico ri-emette il CRS
/// deciso senza conflitti R2.6 con la lineage. Un metadato `geo` non
/// oggetto o non JSON resta invariato: non porta un membro `crs` da
/// sostituire (un `geo` malformato e' gia' errore di discovery, a monte).
pub fn strip_decided_crs_declarations<S: std::hash::BuildHasher>(
    metadata: &mut HashMap<String, String, S>,
) {
    for key in CRS_KEYS_REPLACED_BY_DECISION {
        metadata.remove(key);
    }
    let Some(raw) = metadata.get(GEO_METADATA_KEY).cloned() else {
        return;
    };
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) {
        if let Some(object) = value.as_object_mut() {
            object.remove("crs");
            if object.is_empty() {
                metadata.remove(GEO_METADATA_KEY);
            } else if let Ok(compact) = serde_json::to_string(&value) {
                metadata.insert(GEO_METADATA_KEY.to_owned(), compact);
            }
        }
    }
}

/// Parsing tipizzato di una chiave canonica a enum: assente → `Ok(None)`;
/// presente ma fuori dall'enumerazione chiusa → errore esplicito (R5.1: mai
/// ignorare o correggere; il messaggio non riporta il valore, «errori senza
/// dati» — i messaggi dei tipi di `plenora-core` elencano solo i valori
/// ammessi).
fn parse_canonical_enum<T>(raw: Option<&String>, key: &str) -> Result<Option<T>, PlenoraError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    raw.map(|value| {
        value
            .parse::<T>()
            .map_err(|error| PlenoraError::InvalidPlan(format!("chiave `{key}`: {error}")))
    })
    .transpose()
}

/// Intero decimale senza segno (R5.4): solo cifre ASCII, niente segno (il
/// `FromStr` di `u32` accetterebbe `+`), niente spazi, entro `u32`.
fn parse_unsigned_decimal(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Parsing di una chiave canonica a intero senza segno (R5.4).
fn parse_canonical_u32(raw: Option<&String>, key: &str) -> Result<Option<u32>, PlenoraError> {
    raw.map(|value| {
        parse_unsigned_decimal(value).ok_or_else(|| {
            PlenoraError::InvalidPlan(format!(
                "chiave `{key}`: atteso un intero decimale senza segno (R5.4)"
            ))
        })
    })
    .transpose()
}

/// Encoding canonico di una colonna geometrica (R2.2).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma fuori dall'enum
/// chiuso R3.5 (`wkb` | `ewkb`).
pub fn canonical_geometry_encoding(field: &Field) -> Result<Option<GeometryEncoding>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_ENCODING_KEY),
        PLENORA_GEOMETRY_ENCODING_KEY,
    )
}

/// Dimensionalita' canonica di una colonna geometrica (R2.2; `unknown` e'
/// valore canonico R3.4, mai mappato a `xy`).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non canonica.
pub fn canonical_geometry_dimensions(
    field: &Field,
) -> Result<Option<GeometryDimensions>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_DIMENSIONS_KEY),
        PLENORA_GEOMETRY_DIMENSIONS_KEY,
    )
}

/// Coppia (`types_declaration`, `types`) canonica (R2.2/R3.4.1).
///
/// Le coerenze sono imposte da [`GeometryTypesProperty::from_canonical_list`]
/// (`exact` richiede l'elenco, `unresolved` lo vieta, forma testuale unica).
/// Entrambe le chiavi assenti → `Ok(None)` («proprieta' non dichiarata»,
/// MAI interpretata come `unresolved`); `types` senza `types_declaration` →
/// errore (un produttore conforme emette sempre la dichiarazione, R3.4.1).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la dichiarazione non e' canonica, se `types`
/// compare senza `types_declaration` o se la coppia viola R3.4.1.
pub fn canonical_geometry_types(
    field: &Field,
) -> Result<Option<GeometryTypesProperty>, PlenoraError> {
    let declaration = parse_canonical_enum::<plenora_core::contract::TypesDeclaration>(
        field.metadata().get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY),
        PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
    )?;
    let types = field.metadata().get(PLENORA_GEOMETRY_TYPES_KEY);
    match (declaration, types) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(PlenoraError::InvalidPlan(format!(
            "chiave `{PLENORA_GEOMETRY_TYPES_KEY}` senza \
             `{PLENORA_GEOMETRY_TYPES_DECLARATION_KEY}` (R3.4.1)"
        ))),
        (Some(declaration), types) => {
            // La stringa vuota modella l'elenco assente (chiave non emessa).
            let list = types.map_or("", String::as_str);
            GeometryTypesProperty::from_canonical_list(declaration, list)
                .map(Some)
                .map_err(|error| {
                    PlenoraError::InvalidPlan(format!(
                        "chiavi `{PLENORA_GEOMETRY_TYPES_DECLARATION_KEY}`/`{PLENORA_GEOMETRY_TYPES_KEY}`: {error}"
                    ))
                })
        }
    }
}

/// SRID canonico, se dichiarato (R2.2: intero decimale senza segno, R5.4).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non e' un intero
/// decimale senza segno rappresentabile in `u32`.
pub fn canonical_geometry_srid(field: &Field) -> Result<Option<u32>, PlenoraError> {
    parse_canonical_u32(
        field.metadata().get(PLENORA_GEOMETRY_SRID_KEY),
        PLENORA_GEOMETRY_SRID_KEY,
    )
}

/// Identificatore di autorita' del CRS (R2.2), validato come in
/// plenora-database-tools: non vuoto, entro 1 KiB, senza caratteri di
/// controllo.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma l'identificatore non
/// e' valido.
pub fn canonical_geometry_crs_id(field: &Field) -> Result<Option<String>, PlenoraError> {
    let Some(value) = field.metadata().get(PLENORA_GEOMETRY_CRS_ID_KEY) else {
        return Ok(None);
    };
    if value.is_empty() || value.len() > MAX_CRS_ID_BYTES || value.chars().any(char::is_control) {
        return Err(PlenoraError::InvalidPlan(format!(
            "chiave `{PLENORA_GEOMETRY_CRS_ID_KEY}`: identificatore di autorita' non valido \
             (non vuoto, entro {MAX_CRS_ID_BYTES} byte, senza caratteri di controllo)"
        )));
    }
    Ok(Some(value.clone()))
}

/// Stato di risoluzione del CRS (R2.2).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non canonica.
pub fn canonical_geometry_crs_resolution(
    field: &Field,
) -> Result<Option<CrsResolution>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY),
        PLENORA_GEOMETRY_CRS_RESOLUTION_KEY,
    )
}

/// Definizione CRS testuale e suo formato (R2.2).
///
/// Le due chiavi devono essere presenti insieme, la definizione rispetta i
/// limiti testuali di [`MAX_CRS_DEFINITION_BYTES`] e, se dichiarata
/// `projjson`, deve essere un oggetto JSON (R5.1: una definizione che non
/// rispetta il formato dichiarato e' un valore non canonico, mai
/// reinterpretato).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se una sola delle due chiavi e' presente, se il
/// formato non e' canonico, se la definizione e' vuota/oltre il limite o se
/// dichiara `projjson` senza essere un oggetto JSON.
pub fn canonical_geometry_crs_definition(
    field: &Field,
) -> Result<Option<(String, CrsDefinitionFormat)>, PlenoraError> {
    let definition = field.metadata().get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY);
    let format = parse_canonical_enum::<CrsDefinitionFormat>(
        field.metadata().get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY),
        PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
    )?;
    match (definition, format) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(PlenoraError::InvalidPlan(format!(
            "le chiavi `{PLENORA_GEOMETRY_CRS_DEFINITION_KEY}` e \
             `{PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY}` devono essere presenti insieme (R2.2)"
        ))),
        (Some(definition), Some(format)) => {
            if definition.is_empty()
                || definition.len() > MAX_CRS_DEFINITION_BYTES
                || definition.contains('\0')
            {
                return Err(PlenoraError::InvalidPlan(format!(
                    "chiave `{PLENORA_GEOMETRY_CRS_DEFINITION_KEY}`: definizione non valida \
                     (non vuota, entro {MAX_CRS_DEFINITION_BYTES} byte, senza NUL)"
                )));
            }
            if format == CrsDefinitionFormat::Projjson
                && !matches!(
                    serde_json::from_str::<serde_json::Value>(definition),
                    Ok(serde_json::Value::Object(_))
                )
            {
                return Err(PlenoraError::InvalidPlan(format!(
                    "chiave `{PLENORA_GEOMETRY_CRS_DEFINITION_KEY}`: dichiara `projjson` \
                     ma non e' un oggetto JSON (R5.1)"
                )));
            }
            Ok(Some((definition.clone(), format)))
        }
    }
}

/// Ordine degli assi canonico (R2.2; `unknown` e' valore ammesso).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non canonica.
pub fn canonical_geometry_axis_order(field: &Field) -> Result<Option<AxisOrder>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_AXIS_ORDER_KEY),
        PLENORA_GEOMETRY_AXIS_ORDER_KEY,
    )
}

/// Semantica spaziale canonica (R2.2).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non canonica.
pub fn canonical_geometry_spatial_semantics(
    field: &Field,
) -> Result<Option<SpatialSemantics>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY),
        PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY,
    )
}

/// Precisione delle coordinate canonica (R2.2).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non canonica.
pub fn canonical_geometry_precision(
    field: &Field,
) -> Result<Option<GeometryPrecision>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_PRECISION_KEY),
        PLENORA_GEOMETRY_PRECISION_KEY,
    )
}

/// Identita' logica stabile della colonna (R2.2: intero decimale senza
/// segno, R5.4).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non e' un intero
/// decimale senza segno rappresentabile in `u32`.
pub fn canonical_field_id(field: &Field) -> Result<Option<FieldId>, PlenoraError> {
    Ok(parse_canonical_u32(field.metadata().get(PLENORA_FIELD_ID_KEY), PLENORA_FIELD_ID_KEY)?
        .map(FieldId))
}

/// Versione del protocollo dei metadati dichiarata dallo schema, con il
/// gate R2.5.
///
/// Chiave presente: deve essere un intero decimale senza segno; una
/// versione MAGGIORE di [`PLENORA_CONTRACT_VERSION`] e' rifiutata con
/// errore esplicito (R2.5: mai un'interpretazione parziale). La versione `0`
/// e' accettata: R2.5 impone il fallimento solo per versioni successive a
/// quella nota, e il protocollo `1` e' il primo definito — nessuna versione
/// minore puo' introdurre chiavi che la `1` non conosca.
///
/// Chiave assente: se lo schema (metadati di schema o di qualunque campo)
/// porta chiavi nel namespace `plenora.`, R2.5 la richiede → errore
/// esplicito; senza chiavi canoniche → `Ok(None)` (input legacy o non
/// plenora, nessun protocollo da verificare).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave non e' un intero decimale senza
/// segno o se e' assente in presenza di chiavi canoniche;
/// `PlenoraError::Unsupported` se la versione dichiarata e' successiva a
/// [`PLENORA_CONTRACT_VERSION`].
pub fn read_contract_version(schema: &Schema) -> Result<Option<u32>, PlenoraError> {
    let Some(raw) = schema.metadata().get(PLENORA_CONTRACT_VERSION_KEY) else {
        if schema_has_canonical_keys(schema) {
            return Err(PlenoraError::InvalidPlan(format!(
                "chiavi canoniche `{PLENORA_NAMESPACE_PREFIX}*` senza \
                 `{PLENORA_CONTRACT_VERSION_KEY}` nei metadati dello schema (R2.5)"
            )));
        }
        return Ok(None);
    };
    let version = parse_unsigned_decimal(raw).ok_or_else(|| {
        PlenoraError::InvalidPlan(format!(
            "chiave `{PLENORA_CONTRACT_VERSION_KEY}`: atteso un intero decimale senza segno (R5.4)"
        ))
    })?;
    if version > PLENORA_CONTRACT_VERSION {
        return Err(PlenoraError::Unsupported(format!(
            "`{PLENORA_CONTRACT_VERSION_KEY}` successiva a {PLENORA_CONTRACT_VERSION}: R2.5 \
             impone il fallimento esplicito, mai un'interpretazione parziale"
        )));
    }
    Ok(Some(version))
}

/// Rileva la presenza di chiavi nel namespace canonico (R2.1) nei metadati
/// dello schema o di un qualunque campo (gate R2.5).
fn schema_has_canonical_keys(schema: &Schema) -> bool {
    schema
        .metadata()
        .keys()
        .any(|key| key.starts_with(PLENORA_NAMESPACE_PREFIX))
        || schema.fields().iter().any(|field| {
            field
                .metadata()
                .keys()
                .any(|key| key.starts_with(PLENORA_NAMESPACE_PREFIX))
        })
}

/// Le nozioni geometriche di un campo dopo la lettura delle chiavi
/// canoniche, la verifica di coerenza R2.6 e il completamento per
/// precedenza R2.7 di [`read_geometry_contract_keys`].
///
/// Ogni nozione e' `Option`: assente significa «non dichiarata» (R5.2), mai
/// un default. La provenienza (canonica, legacy o standard esterno) non e'
/// conservata: e' decisa interamente durante la lettura.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalGeometryKeys {
    /// Framing binario delle celle.
    pub encoding: Option<GeometryEncoding>,
    /// Dimensionalita' dichiarata (`unknown` = non risolta, R3.4).
    pub dimensions: Option<GeometryDimensions>,
    /// Coppia (`types_declaration`, `types`) R3.4.1.
    pub types: Option<GeometryTypesProperty>,
    /// SRID, se noto.
    pub srid: Option<u32>,
    /// Identificatore di autorita' del CRS.
    pub crs_id: Option<String>,
    /// Stato di risoluzione del CRS.
    pub crs_resolution: Option<CrsResolution>,
    /// Definizione CRS testuale.
    pub crs_definition: Option<String>,
    /// Formato della definizione CRS.
    pub crs_definition_format: Option<CrsDefinitionFormat>,
    /// Ordine degli assi (`unknown` ammesso).
    pub axis_order: Option<AxisOrder>,
    /// Semantica spaziale.
    pub spatial_semantics: Option<SpatialSemantics>,
    /// Precisione delle coordinate.
    pub precision: Option<GeometryPrecision>,
    /// Identita' logica stabile della colonna.
    pub field_id: Option<FieldId>,
}

/// CRS letto dal metadato legacy `geo`: stringa authority:code oppure
/// oggetto PROJJSON incorporato (la distinzione di [`geo_metadata_json`]).
enum LegacyCrs {
    Id(String),
    Definition(serde_json::Value),
}

/// Le nozioni trasportate dal metadato legacy `geo` (crs, dimensions,
/// encoding), lette in forma STRICT.
struct LegacyGeoKeys {
    crs: Option<LegacyCrs>,
    dimensions: Option<GeometryDimensions>,
    encoding: Option<GeometryEncoding>,
}

/// Lettura STRICT del metadato legacy `geo`: chiave assente o JSON non
/// valido → metadato assente (come i reader esistenti); chiave presente ma
/// valore non canonico → errore esplicito (R5.1 applicato al rango legacy
/// nella lettura di contratto).
fn legacy_geo_keys(field: &Field) -> Result<LegacyGeoKeys, PlenoraError> {
    let encoding = geometry_encoding_from_metadata_strict(field)?;
    let Some(value) = geo_metadata_value(field) else {
        return Ok(LegacyGeoKeys {
            crs: None,
            dimensions: None,
            encoding,
        });
    };
    let crs = match value.get("crs") {
        None => None,
        Some(serde_json::Value::String(text)) => {
            if text.trim().is_empty() {
                return Err(PlenoraError::InvalidPlan(
                    "metadato legacy `geo`: chiave `crs` vuota".to_owned(),
                ));
            }
            Some(LegacyCrs::Id(text.clone()))
        }
        Some(object @ serde_json::Value::Object(_)) => Some(LegacyCrs::Definition(object.clone())),
        Some(_) => {
            return Err(PlenoraError::InvalidPlan(
                "metadato legacy `geo`: chiave `crs` ne' testuale ne' oggetto PROJJSON".to_owned(),
            ));
        }
    };
    let dimensions = match value.get("dimensions") {
        None => None,
        Some(serde_json::Value::String(text)) => Some(text.parse().map_err(|error| {
            PlenoraError::InvalidPlan(format!("metadato legacy `geo`: {error}"))
        })?),
        Some(_) => {
            return Err(PlenoraError::InvalidPlan(
                "metadato legacy `geo`: chiave `dimensions` non testuale".to_owned(),
            ));
        }
    };
    Ok(LegacyGeoKeys {
        crs,
        dimensions,
        encoding,
    })
}

/// Divergenza fra chiavi canoniche e metadato legacy su una nozione (R2.6:
/// il componente fallisce, non sceglie). Il messaggio nomina la nozione, mai
/// i valori («errori senza dati»).
fn divergent_geometry_keys(notion: &str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!(
        "nozione `{notion}` divergente fra chiavi canoniche e metadato legacy `geo` \
         (R2.6: il componente fallisce, non sceglie)"
    ))
}

/// Lettura di contratto di un campo geometria.
///
/// Chiavi canoniche (fail-closed per chiave), coerenza con il metadato
/// legacy `geo` (R2.6) e completamento per precedenza canonica > legacy >
/// standard esterno (R2.7).
///
/// Protocollo:
///
/// 1. ogni chiave canonica e' parsata dal suo reader tipizzato: assente →
///    `None`, valore non canonico → errore (R5.1);
/// 2. coerenze FRA chiavi canoniche: `axis_order` obbligatorio se `crs_id` o
///    `crs_definition` e' presente (tabella §2, valore `unknown` ammesso);
///    `crs_resolution = missing` non ammette `crs_id`/`crs_definition`/
///    `srid`/`axis_order` (coerenza gia' imposta da plenora-database-tools);
/// 3. coerenza R2.6 con il legacy: ogni nozione presente in DUE
///    rappresentazioni deve coincidere — `encoding` e `dimensions` a parita'
///    di valore canonico, il CRS a parita' di forma (stringa
///    authority:code ↔ `crs_id`, oggetto PROJJSON ↔ `crs_definition` con
///    formato `projjson`, confronto per valore JSON); forme non confrontabili
///    (es. `crs_id` canonico contro oggetto legacy) contano come divergenza:
///    il componente fallisce, non sceglie;
/// 4. completamento R2.7 (mai arbitrato, decidibile senza ispezionare i
///    dati): una nozione assente nel rango canonico e' adottata dal legacy;
///    se assente anche li, il solo standard esterno `ARROW:extension:name =
///    geoarrow.wkb` completa `encoding` con `wkb`. Nessun controllo di
///    coerenza fra encoding e nome di estensione: `geoarrow.wkb`
///    dichiara la famiglia binaria WKB (di cui EWKB e' il dialetto con
///    SRID) e i costruttori di questo modulo emettono legittimamente
///    `ewkb` sotto quel nome.
///
/// Il gate di versione R2.5 NON e' applicato qui: la versione vive nei
/// metadati dello schema ([`read_contract_version`]), non nel campo, e
/// resta responsabilita' del chiamante.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per valori canonici non validi, coerenze
/// intra-canoniche violate o divergenza canonica-vs-legacy (R2.6);
/// `PlenoraError::Unsupported` per un encoding legacy non rappresentabile
/// (R3.5, come [`geometry_encoding_from_metadata_strict`]).
pub fn read_geometry_contract_keys(field: &Field) -> Result<CanonicalGeometryKeys, PlenoraError> {
    let (crs_definition, crs_definition_format) =
        match canonical_geometry_crs_definition(field)? {
            Some((definition, format)) => (Some(definition), Some(format)),
            None => (None, None),
        };
    let mut keys = CanonicalGeometryKeys {
        encoding: canonical_geometry_encoding(field)?,
        dimensions: canonical_geometry_dimensions(field)?,
        types: canonical_geometry_types(field)?,
        srid: canonical_geometry_srid(field)?,
        crs_id: canonical_geometry_crs_id(field)?,
        crs_resolution: canonical_geometry_crs_resolution(field)?,
        crs_definition,
        crs_definition_format,
        axis_order: canonical_geometry_axis_order(field)?,
        spatial_semantics: canonical_geometry_spatial_semantics(field)?,
        precision: canonical_geometry_precision(field)?,
        field_id: canonical_field_id(field)?,
    };

    // Coerenze fra chiavi canoniche (tabella §2), verificate PRIMA del
    // completamento: riguardano la sola rappresentazione canonica, cosi' un
    // input legacy senza `axis_order` resta leggibile.
    if (keys.crs_id.is_some() || keys.crs_definition.is_some()) && keys.axis_order.is_none() {
        return Err(PlenoraError::InvalidPlan(format!(
            "chiave `{PLENORA_GEOMETRY_AXIS_ORDER_KEY}` obbligatoria quando \
             `{PLENORA_GEOMETRY_CRS_ID_KEY}` o `{PLENORA_GEOMETRY_CRS_DEFINITION_KEY}` \
             e' presente (tabella R2.2; valore `unknown` ammesso)"
        )));
    }
    if keys.crs_resolution == Some(CrsResolution::Missing)
        && (keys.crs_id.is_some()
            || keys.crs_definition.is_some()
            || keys.srid.is_some()
            || keys.axis_order.is_some())
    {
        return Err(PlenoraError::InvalidPlan(format!(
            "`{PLENORA_GEOMETRY_CRS_RESOLUTION_KEY}` = `missing` non ammette metadati CRS \
             dichiarati (R2.2)"
        )));
    }

    let legacy = legacy_geo_keys(field)?;

    // Coerenza R2.6 + completamento R2.7 per nozione.
    if let (Some(canonical), Some(legacy_encoding)) = (keys.encoding, legacy.encoding) {
        if canonical != legacy_encoding {
            return Err(divergent_geometry_keys("encoding"));
        }
    }
    if keys.encoding.is_none() {
        keys.encoding = legacy.encoding;
    }
    if keys.encoding.is_none()
        && field.metadata().get(GEOARROW_EXTENSION_KEY).map(String::as_str)
            == Some(GEOARROW_WKB_EXTENSION)
    {
        // Standard esterno (R2.7, ultimo rango): il nome di estensione
        // dichiara la famiglia WKB.
        keys.encoding = Some(GeometryEncoding::Wkb);
    }

    if let (Some(canonical), Some(legacy_dimensions)) = (keys.dimensions, legacy.dimensions) {
        if canonical != legacy_dimensions {
            return Err(divergent_geometry_keys("dimensions"));
        }
    }
    if keys.dimensions.is_none() {
        keys.dimensions = legacy.dimensions;
    }

    if let Some(legacy_crs) = legacy.crs {
        if keys.crs_resolution == Some(CrsResolution::Missing) {
            // Il canonico dichiara «CRS mancante», il legacy dichiara un
            // CRS: divergenza (R2.6).
            return Err(divergent_geometry_keys("crs"));
        }
        if keys.crs_id.is_none() && keys.crs_definition.is_none() {
            // Completamento R2.7 dal rango legacy.
            match legacy_crs {
                LegacyCrs::Id(id) => keys.crs_id = Some(id),
                LegacyCrs::Definition(value) => {
                    keys.crs_definition = Some(value.to_string());
                    keys.crs_definition_format = Some(CrsDefinitionFormat::Projjson);
                }
            }
        } else {
            let coherent = match &legacy_crs {
                LegacyCrs::Id(legacy_id) => keys.crs_id.as_ref() == Some(legacy_id),
                LegacyCrs::Definition(legacy_value) => {
                    keys.crs_definition_format == Some(CrsDefinitionFormat::Projjson)
                        && keys
                            .crs_definition
                            .as_deref()
                            .and_then(|definition| {
                                serde_json::from_str::<serde_json::Value>(definition).ok()
                            })
                            .as_ref()
                            == Some(legacy_value)
                }
            };
            if !coherent {
                return Err(divergent_geometry_keys("crs"));
            }
        }
    }

    Ok(keys)
}

/// Colonna geometria di un batch, gia' indicizzata da
/// [`geometry_column_index`]: deve essere un `BinaryArray`.
///
/// # Errors
///
/// `PlenoraError::Schema` se la colonna all'indice `geometry_index` non e'
/// un `BinaryArray` (schema incoerente con l'indice calcolato).
pub fn batch_geometry_cells<'a>(
    batch: &'a RecordBatch,
    geometry_index: usize,
    geometry_column: &str,
) -> Result<&'a BinaryArray, PlenoraError> {
    batch
        .column(geometry_index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| {
            geometry_column_not_binary(
                geometry_column,
                batch.column(geometry_index).data_type(),
            )
        })
}

/// Decodifica una cella WKB non-null: il limite per cella e' applicato prima
/// di toccare i dati, poi vale il contratto WKB strutturale del kernel.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il payload supera [`MAX_CELL_BYTES`]; in piu'
/// gli errori di [`geometry_from_wkb`] (contratto WKB strutturale e
/// validazione OGC).
pub fn decode_geometry_cell(payload: &[u8]) -> Result<Geometry<f64>, PlenoraError> {
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(cell_too_large(payload.len() as u64));
    }
    geometry_from_wkb(payload)
}

/// Codifica una geometria gia' validata dal kernel in WKB 2D entro il limite
/// per cella.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la serializzazione WKB della geometria
/// fallisce o se il payload prodotto supera [`MAX_CELL_BYTES`].
pub fn encode_geometry(geometry: &Geometry<f64>) -> Result<Vec<u8>, PlenoraError> {
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| PlenoraError::InvalidPlan(format!("geometria prodotta non valida: {error}")))?;
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(cell_too_large(payload.len() as u64));
    }
    Ok(payload)
}

/// Applica `f` a ogni cella non-null preservando i null; il limite per cella
/// e' applicato prima di toccare i dati.
///
/// Le righe sono indipendenti: l'iterazione e' parallela (rayon) con collect
/// indicizzato, quindi l'ordine dell'output resta deterministico.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se una cella non-null supera [`MAX_CELL_BYTES`];
/// in piu' l'errore restituito da `f` sulla prima cella IN ORDINE DI RIGA
/// che fallisce (deterministico, ADR-0001).
pub fn map_nullable<T: Send>(
    cells: &BinaryArray,
    f: impl Fn(&[u8]) -> Result<Option<T>, PlenoraError> + Sync,
) -> Result<Vec<Option<T>>, PlenoraError> {
    let cell_values: Vec<Option<&[u8]>> = cells.iter().collect();
    // ADR-0001: il collect parallelo di `Result` sceglie l'errore in modo
    // NON deterministico (il primo che acquisisce il mutex interno di
    // rayon). Qui i `Result` sono raccolti per riga (l'ordine del collect
    // parallelo indicizzato e' preservato) e il primo errore IN ORDINE DI
    // RIGA e' selezionato dal collect sequenziale: stesso input, stesso
    // errore, sempre. L'identita' dell'errore e' output.
    let results: Vec<Result<Option<T>, PlenoraError>> = cell_values
        .into_par_iter()
        .map(|cell| match cell {
            None => Ok(None),
            Some(payload) => {
                if payload.len() as u64 > MAX_CELL_BYTES {
                    return Err(cell_too_large(payload.len() as u64));
                }
                f(payload)
            }
        })
        .collect();
    results.into_iter().collect()
}

/// STIMA dei byte nativi delle geometrie decodificate di una colonna
/// geometria (ADR-0002, Fase 2B-M2b).
///
/// Decodifica ogni cella non-null e somma le stime per cella. Il valore e'
/// una STIMA dichiarata (formula in [`crate::memory_estimate`]), da riportare
/// nelle metriche come "memoria nativa stimata", mai come conteggio preciso.
/// I null contribuiscono zero.
///
/// # Errors
///
/// Come [`decode_geometry_cell`], propagato via [`map_nullable`]
/// ([`MAX_CELL_BYTES`] e contratto WKB strutturale del kernel).
pub fn estimate_decoded_cells_native_bytes(cells: &BinaryArray) -> Result<u64, PlenoraError> {
    let estimates = map_nullable(cells, |payload| {
        decode_geometry_cell(payload).map(|geometry| Some(estimate_geometry_native_bytes(&geometry)))
    })?;
    Ok(estimates
        .iter()
        .flatten()
        .fold(0_u64, |total, estimate| total.saturating_add(*estimate)))
}

/// Come [`estimate_decoded_cells_native_bytes`], ma accumula ogni STIMA di
/// cella decodificata in `accumulator`.
///
/// Punto naturale di raccolta della metrica "stimata" per il governor;
/// l'integrazione con `plenora-engine` e' volutamente rimandata. Restituisce
/// il totale corrente dell'accumulatore, non il solo contributo di questa
/// colonna.
///
/// # Errors
///
/// Come [`decode_geometry_cell`], propagato via [`map_nullable`]
/// ([`MAX_CELL_BYTES`] e contratto WKB strutturale del kernel).
pub fn accumulate_decoded_cells_native_bytes(
    cells: &BinaryArray,
    accumulator: &DecodedNativeBytesEstimate,
) -> Result<u64, PlenoraError> {
    map_nullable(cells, |payload| {
        decode_geometry_cell(payload).map(|geometry| {
            accumulator.record(&geometry);
            Some(())
        })
    })?;
    Ok(accumulator.total())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;
    use plenora_core::arrow::RecordBatch;
    use plenora_core::contract::{
        ContractCrs, ContractProperty, GeometryType, PropertyConfidence, PropertyScope,
        TypesDeclaration,
    };
    use plenora_core::crs::{CrsKind, ResolvedCrs};
    use std::sync::Arc;

    const CRS: &str = "EPSG:3857";

    fn fixture_batch(cells: &[Option<&Vec<u8>>]) -> (Schema, RecordBatch) {
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("geometry field"),
        ]);
        let ids = plenora_core::arrow::array::Int64Array::from_iter_values(
            0..i64::try_from(cells.len()).expect("fixture entro i64"),
        );
        let geometry: BinaryArray = cells.iter().map(|cell| cell.map(Vec::as_slice)).collect();
        let batch = RecordBatch::try_new(
            Arc::new(schema.clone()),
            vec![Arc::new(ids), Arc::new(geometry)],
        )
        .expect("batch");
        (schema, batch)
    }

    fn square_wkb(size: f64) -> Vec<u8> {
        encode_geometry(&Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: size, y: 0.0),
            (x: size, y: size), (x: 0.0, y: size),
            (x: 0.0, y: 0.0),
        ]))
        .expect("encode")
    }

    #[test]
    fn geo_metadata_embeds_projjson_objects_and_plain_codes() {
        let code = geo_metadata_json(CRS).expect("code");
        let parsed: serde_json::Value = serde_json::from_str(&code).unwrap();
        assert_eq!(parsed["crs"], serde_json::Value::String(CRS.to_owned()));

        let projjson = r#"{"type":"ProjectedCRS","name":"demo"}"#;
        let embedded = geo_metadata_json(projjson).expect("projjson");
        let parsed: serde_json::Value = serde_json::from_str(&embedded).unwrap();
        assert_eq!(parsed["crs"]["type"], "ProjectedCRS");

        assert!(matches!(
            geo_metadata_json("  "),
            Err(PlenoraError::Crs(_))
        ));
        let oversized = "X".repeat(MAX_CRS_DEFINITION_BYTES + 1);
        assert!(matches!(
            geo_metadata_json(&oversized),
            Err(PlenoraError::Crs(_))
        ));
    }

    #[test]
    fn geometry_output_field_carries_extension_and_crs_metadata() {
        let field = geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("field");
        assert_eq!(field.data_type(), &DataType::Binary);
        assert_eq!(
            field
                .metadata()
                .get(GEOARROW_EXTENSION_KEY)
                .map(String::as_str),
            Some(GEOARROW_WKB_EXTENSION)
        );
        let geo: serde_json::Value = serde_json::from_str(
            field
                .metadata()
                .get(GEO_METADATA_KEY)
                .expect("geo metadata"),
        )
        .expect("geo JSON");
        assert_eq!(geo.get("crs").and_then(serde_json::Value::as_str), Some(CRS));
        // B1.1: la scrittura dichiara sempre la dimensionalita' (Xy dai
        // costruttori attuali; la propagazione reale e' B1.3).
        assert_eq!(
            geo.get("dimensions").and_then(serde_json::Value::as_str),
            Some("xy")
        );
    }

    #[test]
    fn geo_metadata_with_dimensions_embeds_icd_string() {
        for (dimensions, text) in [
            (GeometryDimensions::Xy, "xy"),
            (GeometryDimensions::Xyz, "xyz"),
            (GeometryDimensions::Xym, "xym"),
            (GeometryDimensions::Xyzm, "xyzm"),
            (GeometryDimensions::Unknown, "unknown"),
        ] {
            let json = geo_metadata_json_with_dimensions(CRS, dimensions).expect("metadata");
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                parsed.get("dimensions").and_then(serde_json::Value::as_str),
                Some(text)
            );
            assert_eq!(
                parsed.get("crs").and_then(serde_json::Value::as_str),
                Some(CRS)
            );
        }
        // Le validazioni CRS restano quelle di `geo_metadata_json`.
        assert!(matches!(
            geo_metadata_json_with_dimensions("  ", GeometryDimensions::Xy),
            Err(PlenoraError::Crs(_))
        ));
    }

    #[test]
    fn geo_metadata_with_encoding_writes_the_key_only_when_declared() {
        // B1.4: `Some` -> chiave `encoding` in forma ICD; `None` -> chiave
        // omessa e JSON identico byte-per-byte alla forma senza encoding
        // (fingerprint e retrocompatibilita' invariati).
        for encoding in [GeometryEncoding::Wkb, GeometryEncoding::Ewkb] {
            let json = geo_metadata_json_with_encoding(CRS, GeometryDimensions::Xy, Some(encoding))
                .expect("metadata");
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                parsed.get("encoding").and_then(serde_json::Value::as_str),
                Some(encoding.as_str())
            );
            assert_eq!(
                parsed.get("dimensions").and_then(serde_json::Value::as_str),
                Some("xy")
            );
        }
        let without = geo_metadata_json_with_encoding(CRS, GeometryDimensions::Xyz, None)
            .expect("metadata senza encoding");
        assert_eq!(
            without,
            geo_metadata_json_with_dimensions(CRS, GeometryDimensions::Xyz).expect("dimensions"),
            "None: byte-per-byte identico alla forma pre-B1.4"
        );
        let parsed: serde_json::Value = serde_json::from_str(&without).unwrap();
        assert!(parsed.get("encoding").is_none(), "chiave omessa con None");

        // Il campo di output rilegge l'encoding dichiarato (round-trip con
        // il reader della discovery); con None il reader ottiene None.
        let field = geometry_output_field_with_encoding(
            DEFAULT_GEOMETRY_COLUMN,
            CRS,
            GeometryDimensions::Xy,
            Some(GeometryEncoding::Ewkb),
        )
        .expect("field");
        assert_eq!(
            geometry_encoding_from_metadata(&field),
            Some(GeometryEncoding::Ewkb)
        );
        assert_eq!(
            geometry_dimensions_from_metadata(&field),
            GeometryDimensions::Xy
        );
        let field_none =
            geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("field senza encoding");
        assert_eq!(geometry_encoding_from_metadata(&field_none), None);
    }

    #[test]
    fn dimensions_and_encoding_readers_never_default_to_xy() {
        // Campo senza metadato `geo`: Unknown/None, mai default xy (R3.4).
        let bare = Field::new("geom", DataType::Binary, true);
        assert_eq!(
            geometry_dimensions_from_metadata(&bare),
            GeometryDimensions::Unknown
        );
        assert_eq!(geometry_encoding_from_metadata(&bare), None);

        // Metadato `geo` con la sola chiave `crs` (formato pre-B1.1):
        // dimensionalita' non risolta -> Unknown.
        let mut metadata = HashMap::new();
        metadata.insert(GEO_METADATA_KEY.to_owned(), geo_metadata_json(CRS).unwrap());
        let crs_only = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
        assert_eq!(
            geometry_dimensions_from_metadata(&crs_only),
            GeometryDimensions::Unknown
        );
        assert_eq!(geometry_encoding_from_metadata(&crs_only), None);

        // Round-trip: scrittura con dimensions + encoding -> lettura.
        let mut metadata = HashMap::new();
        metadata.insert(
            GEO_METADATA_KEY.to_owned(),
            r#"{"crs":"EPSG:3857","dimensions":"xyz","encoding":"ewkb"}"#.to_owned(),
        );
        let declared = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
        assert_eq!(
            geometry_dimensions_from_metadata(&declared),
            GeometryDimensions::Xyz
        );
        assert_eq!(
            geometry_encoding_from_metadata(&declared),
            Some(GeometryEncoding::Ewkb)
        );

        // Valori non riconosciuti o JSON rotto: Unknown/None, mai default.
        for raw in [
            r#"{"crs":"EPSG:3857","dimensions":"2d","encoding":"twkb"}"#,
            "non json",
            r#"{"dimensions":42}"#,
        ] {
            let mut metadata = HashMap::new();
            metadata.insert(GEO_METADATA_KEY.to_owned(), raw.to_owned());
            let field = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
            assert_eq!(
                geometry_dimensions_from_metadata(&field),
                GeometryDimensions::Unknown
            );
            assert_eq!(geometry_encoding_from_metadata(&field), None);
        }
    }

    #[test]
    fn strict_encoding_reader_rejects_unrepresentable_framing() {
        // Discovery (B1.3): encoding fuori dall'enum chiuso -> errore
        // esplicito (R3.5), mai mappato o ignorato.
        for raw in [
            r#"{"crs":"EPSG:3857","encoding":"gpkg"}"#,
            r#"{"crs":"EPSG:3857","encoding":"twkb"}"#,
            r#"{"crs":"EPSG:3857","encoding":42}"#,
            r#"{"crs":"EPSG:3857","encoding":"WKB"}"#,
        ] {
            let mut metadata = HashMap::new();
            metadata.insert(GEO_METADATA_KEY.to_owned(), raw.to_owned());
            let field = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
            assert!(matches!(
                geometry_encoding_from_metadata_strict(&field),
                Err(PlenoraError::Unsupported(_))
            ));
        }

        // Encoding rappresentabili -> Ok(Some); assente o metadato rotto ->
        // Ok(None) (come il reader leniente).
        for (raw, expected) in [
            (r#"{"crs":"EPSG:3857","encoding":"wkb"}"#, Some(GeometryEncoding::Wkb)),
            (r#"{"crs":"EPSG:3857","encoding":"ewkb"}"#, Some(GeometryEncoding::Ewkb)),
        ] {
            let mut metadata = HashMap::new();
            metadata.insert(GEO_METADATA_KEY.to_owned(), raw.to_owned());
            let field = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
            assert_eq!(geometry_encoding_from_metadata_strict(&field).unwrap(), expected);
        }
        let bare = Field::new("geom", DataType::Binary, true);
        assert_eq!(geometry_encoding_from_metadata_strict(&bare).unwrap(), None);
        let mut metadata = HashMap::new();
        metadata.insert(GEO_METADATA_KEY.to_owned(), "non json".to_owned());
        let broken = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
        assert_eq!(geometry_encoding_from_metadata_strict(&broken).unwrap(), None);
        let mut metadata = HashMap::new();
        metadata.insert(GEO_METADATA_KEY.to_owned(), geo_metadata_json(CRS).unwrap());
        let crs_only = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
        assert_eq!(geometry_encoding_from_metadata_strict(&crs_only).unwrap(), None);
    }

    #[test]
    fn geometry_column_index_enforces_binary_type_and_extension_metadata() {
        let (schema, _) = fixture_batch(&[]);
        assert_eq!(
            geometry_column_index(&schema, DEFAULT_GEOMETRY_COLUMN).unwrap(),
            1
        );
        assert!(matches!(
            geometry_column_index(&schema, "assente"),
            Err(PlenoraError::Schema(_))
        ));
        assert!(matches!(
            geometry_column_index(&schema, "id"),
            Err(PlenoraError::Schema(_))
        ));

        let no_metadata = Schema::new(vec![Field::new(
            DEFAULT_GEOMETRY_COLUMN,
            DataType::Binary,
            true,
        )]);
        assert!(matches!(
            geometry_column_index(&no_metadata, DEFAULT_GEOMETRY_COLUMN),
            Err(PlenoraError::Schema(_))
        ));
    }

    #[test]
    fn cell_roundtrip_preserves_nulls_and_enforces_cell_limit() {
        let square = square_wkb(4.0);
        let (schema, batch) = fixture_batch(&[Some(&square), None, Some(&square)]);
        let index = geometry_column_index(&schema, DEFAULT_GEOMETRY_COLUMN).unwrap();
        let cells = batch_geometry_cells(&batch, index, DEFAULT_GEOMETRY_COLUMN).unwrap();

        let decoded = map_nullable(cells, |payload| {
            decode_geometry_cell(payload).and_then(|geometry| encode_geometry(&geometry).map(Some))
        })
        .expect("roundtrip");
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].as_deref(), Some(square.as_slice()));
        assert!(decoded[1].is_none());
        assert_eq!(decoded[2].as_deref(), Some(square.as_slice()));

        let oversized = vec![
            0_u8;
            usize::try_from(MAX_CELL_BYTES + 1).expect("64 MiB + 1 entra in usize")
        ];
        let cells = BinaryArray::from_iter([Some(oversized.as_slice())]);
        assert!(matches!(
            map_nullable(&cells, |payload| decode_geometry_cell(payload).map(Some)),
            Err(PlenoraError::InvalidPlan(_))
        ));
        assert!(matches!(
            decode_geometry_cell(&oversized),
            Err(PlenoraError::InvalidPlan(_))
        ));
    }

    #[test]
    fn map_nullable_reports_the_first_failing_row_deterministically() {
        // ADR-0001: con piu' celle fallite, l'errore riportato DEVE essere
        // quello della prima riga in ordine, a qualunque scheduling rayon
        // (il collect parallelo diretto sceglierebbe il primo errore che
        // acquisisce il mutex interno: non deterministico).
        let cells: BinaryArray = (0_u8..64).map(|row| Some(vec![row])).collect();
        for attempt in 0..50 {
            let result = map_nullable(&cells, |payload| {
                let row = payload[0];
                if row == 3 || row == 7 || row == 41 {
                    Err(PlenoraError::InvalidPlan(format!("fallimento riga {row}")))
                } else {
                    Ok(Some(()))
                }
            });
            let Err(PlenoraError::InvalidPlan(message)) = &result else {
                panic!("tentativo {attempt}: atteso errore, ottenuto {result:?}");
            };
            assert_eq!(message, "fallimento riga 3", "tentativo {attempt}");
        }
    }

    #[test]
    fn batch_geometry_cells_rejects_non_binary_columns() {
        let schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
        let ids = plenora_core::arrow::array::Int64Array::from_iter_values(0..2_i64);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(ids)]).expect("batch");
        assert!(matches!(
            batch_geometry_cells(&batch, 0, "id"),
            Err(PlenoraError::Schema(_))
        ));
    }

    /// STIMA per colonna geometria (ADR-0002): somma delle stime delle celle
    /// non-null; i null contribuiscono zero; l'accumulatore riporta lo
    /// stesso totale come metrica "stimata".
    #[test]
    fn decoded_cells_estimate_sums_non_null_cells_and_feeds_the_accumulator() {
        use crate::memory_estimate::{estimate_geometry_native_bytes, DecodedNativeBytesEstimate};

        let square = Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: 4.0, y: 0.0),
            (x: 4.0, y: 4.0), (x: 0.0, y: 4.0),
            (x: 0.0, y: 0.0),
        ]);
        let point = Geometry::Point(geo::Point::new(1.0, 2.0));
        let cells_payload = [
            Some(encode_geometry(&square).expect("square")),
            None,
            Some(encode_geometry(&point).expect("point")),
        ];
        let cells: BinaryArray = cells_payload.iter().map(|cell| cell.as_deref()).collect();

        let expected = estimate_geometry_native_bytes(&square)
            + estimate_geometry_native_bytes(&point);
        assert_eq!(
            estimate_decoded_cells_native_bytes(&cells).expect("estimate"),
            expected
        );

        let accumulator = DecodedNativeBytesEstimate::new();
        let total =
            accumulate_decoded_cells_native_bytes(&cells, &accumulator).expect("accumulate");
        assert_eq!(total, expected);
        assert_eq!(accumulator.total(), expected);
        // Secondo passaggio: l'accumulatore continua a crescere (metrica
        // cumulativa), la stima per colonna resta puntuale.
        accumulate_decoded_cells_native_bytes(&cells, &accumulator).expect("accumulate");
        assert_eq!(accumulator.total(), 2 * expected);
    }

    // ------------------------------------------------------------------
    // Milestone B — protocollo delle chiavi canoniche (R2.x, R3.4.1, R5.x)
    // ------------------------------------------------------------------

    fn resolved_crs(definition: &str) -> ResolvedCrs {
        ResolvedCrs::from_resolved_parts(
            definition.to_owned(),
            serde_json::json!({"type": "ProjectedCRS"}),
            CrsKind::Projected,
            Some(1.0),
        )
    }

    fn full_contract() -> GeometryColumnContract {
        GeometryColumnContract {
            field_id: FieldId(7),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(resolved_crs("EPSG:3857")),
            dimensions: GeometryDimensions::Xyz,
            encoding: Some(GeometryEncoding::Wkb),
            nullable: true,
            types: ContractProperty::new(
                PropertyConfidence::Declared(
                    // Non in ordine canonico apposta: l'emissione normalizza.
                    GeometryTypesProperty::new(
                        TypesDeclaration::Exact,
                        vec![GeometryType::Polygon, GeometryType::Point],
                    )
                    .expect("coerente"),
                ),
                PropertyScope::Dataset,
            ),
        }
    }

    fn full_details() -> GeometryMetadataDetails {
        GeometryMetadataDetails {
            axis_order: Some(AxisOrder::EastingNorthing),
            srid: Some(3857),
            spatial_semantics: Some(SpatialSemantics::Geometry),
            precision: Some(GeometryPrecision::Float64),
        }
    }

    fn field_with_pairs(pairs: &[(&str, &str)]) -> Field {
        Field::new("geom", DataType::Binary, true).with_metadata(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn canonical_metadata_emits_the_full_contract() {
        let metadata = canonical_geometry_metadata(&full_contract(), &full_details());
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(get(PLENORA_GEOMETRY_ENCODING_KEY), Some("wkb"));
        assert_eq!(get(PLENORA_GEOMETRY_DIMENSIONS_KEY), Some("xyz"));
        assert_eq!(get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY), Some("exact"));
        // Elenco normalizzato: unici, ordine canonico §3.1.
        assert_eq!(get(PLENORA_GEOMETRY_TYPES_KEY), Some("point,polygon"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY), Some("resolved"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), Some("EPSG:3857"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY), None);
        assert_eq!(get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY), None);
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("easting_northing"));
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), Some("3857"));
        assert_eq!(get(PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY), Some("geometry"));
        assert_eq!(get(PLENORA_GEOMETRY_PRECISION_KEY), Some("float64"));
        // `field_id` non e' emesso (R2.2 opzionale; il FieldId di grafo non
        // ha significato fuori dal processo, ADR-0009 decisione 3).
        assert_eq!(get(PLENORA_FIELD_ID_KEY), None);
    }

    #[test]
    fn canonical_metadata_minimal_omits_everything_not_declared() {
        let contract = GeometryColumnContract {
            types: GeometryColumnContract::undeclared_types(),
            encoding: None,
            ..full_contract()
        };
        let metadata = canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
        let get = |key: &str| metadata.get(key).map(String::as_str);
        // Obbligatorie e oneste: dimensions, crs_resolution, crs_id,
        // axis_order = `unknown` (valore canonico, la chiave e' obbligatoria
        // quando un CRS e' presente).
        assert_eq!(get(PLENORA_GEOMETRY_DIMENSIONS_KEY), Some("xyz"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY), Some("resolved"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), Some("EPSG:3857"));
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("unknown"));
        // `field_id` non e' emesso (R2.2 opzionale; il FieldId di grafo non
        // ha significato fuori dal processo, ADR-0009 decisione 3).
        assert_eq!(get(PLENORA_FIELD_ID_KEY), None);
        // R5.2 + R3.4.1: le opzionali e le non dichiarate restano assenti.
        for key in [
            PLENORA_GEOMETRY_ENCODING_KEY,
            PLENORA_GEOMETRY_TYPES_KEY,
            PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
            PLENORA_GEOMETRY_SRID_KEY,
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
            PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY,
            PLENORA_GEOMETRY_PRECISION_KEY,
        ] {
            assert_eq!(get(key), None, "{key} deve restare assente");
        }
    }

    #[test]
    fn canonical_metadata_declared_unresolved_reemits_the_original_declarations() {
        // R4.6.4: l'incoerenza dichiarata arriva al bordo di scrittura con
        // le dichiarazioni ORIGINALI ri-emesse invariate — mai una persa,
        // mai una inventata. `srid` non e' ri-emesso dal blocco (resta alla
        // lineage); `axis_order` segue la stessa regola del risolto
        // (obbligatorio con una rappresentazione presente, `unknown` come
        // assenza dichiarata).
        let contract = GeometryColumnContract {
            crs: ContractCrs::DeclaredUnresolved {
                crs_id: Some("EPSG:99999".to_owned()),
                definition: None,
                definition_format: None,
            },
            types: GeometryColumnContract::undeclared_types(),
            encoding: None,
            ..full_contract()
        };
        let metadata = canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY),
            Some("declared_unresolved")
        );
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), Some("EPSG:99999"));
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("unknown"));
        for key in [
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY,
            PLENORA_GEOMETRY_SRID_KEY,
        ] {
            assert_eq!(get(key), None, "{key} non dichiarato -> assente");
        }

        // Definizione con il suo formato (R4.3): entrambi ri-emessi.
        let contract = GeometryColumnContract {
            crs: ContractCrs::DeclaredUnresolved {
                crs_id: Some("EPSG:4326".to_owned()),
                definition: Some(r#"{"type":"GeographicCRS"}"#.to_owned()),
                definition_format: Some(CrsDefinitionFormat::Projjson),
            },
            types: GeometryColumnContract::undeclared_types(),
            encoding: None,
            ..full_contract()
        };
        let metadata = canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY),
            Some("declared_unresolved")
        );
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), Some("EPSG:4326"));
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY),
            Some(r#"{"type":"GeographicCRS"}"#)
        );
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY),
            Some("projjson")
        );
    }

    #[test]
    fn canonical_metadata_types_follow_the_confidence() {
        // Qualunque confidence con valore (Declared/Proven/Estimated) emette
        // la coppia; `unresolved` emette la sola dichiarazione (elenco vietato
        // da R3.4.1); confidence `Unknown` non emette nulla (mai inventare).
        for confidence in [
            PropertyConfidence::Estimated(
                GeometryTypesProperty::new(TypesDeclaration::Mixed, Vec::new()).expect("coerente"),
            ),
            PropertyConfidence::Proven(
                GeometryTypesProperty::new(TypesDeclaration::Unresolved, Vec::new())
                    .expect("coerente"),
            ),
        ] {
            let contract = GeometryColumnContract {
                types: ContractProperty::new(confidence, PropertyScope::Schema),
                ..full_contract()
            };
            let metadata =
                canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
            assert!(metadata.contains_key(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY));
        }
        let unresolved = canonical_geometry_metadata(
            &GeometryColumnContract {
                types: ContractProperty::new(
                    PropertyConfidence::Declared(
                        GeometryTypesProperty::new(TypesDeclaration::Unresolved, Vec::new())
                            .expect("coerente"),
                    ),
                    PropertyScope::Schema,
                ),
                ..full_contract()
            },
            &GeometryMetadataDetails::default(),
        );
        assert_eq!(
            unresolved
                .get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY)
                .map(String::as_str),
            Some("unresolved")
        );
        assert!(
            !unresolved.contains_key(PLENORA_GEOMETRY_TYPES_KEY),
            "`unresolved` non porta elenco: la chiave `types` non e' emessa"
        );
    }

    #[test]
    fn canonical_metadata_emits_projjson_definitions_as_crs_definition() {
        let projjson = r#"{"type":"ProjectedCRS","name":"demo"}"#;
        let contract = GeometryColumnContract {
            crs: ContractCrs::Resolved(resolved_crs(projjson)),
            ..full_contract()
        };
        let metadata = canonical_geometry_metadata(&contract, &full_details());
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY), Some(projjson));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY), Some("projjson"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), None);
    }

    #[test]
    fn contract_keys_roundtrip_emit_then_read() {
        let field = Field::new("geom", DataType::Binary, true)
            .with_metadata(canonical_geometry_metadata(&full_contract(), &full_details()));
        let keys = read_geometry_contract_keys(&field).expect("read");
        assert_eq!(keys.encoding, Some(GeometryEncoding::Wkb));
        assert_eq!(keys.dimensions, Some(GeometryDimensions::Xyz));
        let types = keys.types.as_ref().expect("types");
        assert_eq!(types.declaration(), TypesDeclaration::Exact);
        assert_eq!(types.to_canonical_list(), "point,polygon");
        assert_eq!(keys.srid, Some(3857));
        assert_eq!(keys.crs_id.as_deref(), Some("EPSG:3857"));
        assert_eq!(keys.crs_resolution, Some(CrsResolution::Resolved));
        assert_eq!(keys.crs_definition, None);
        assert_eq!(keys.crs_definition_format, None);
        assert_eq!(keys.axis_order, Some(AxisOrder::EastingNorthing));
        assert_eq!(keys.spatial_semantics, Some(SpatialSemantics::Geometry));
        assert_eq!(keys.precision, Some(GeometryPrecision::Float64));
        // `field_id` non e' emesso dal contratto (R2.2 opzionale)...
        assert_eq!(keys.field_id, None);
        // ...ma una chiave RICEVUTA e' propagata invariata (R2.4).
        let mut received = Field::new("geom", DataType::Binary, true)
            .with_metadata(canonical_geometry_metadata(&full_contract(), &full_details()))
            .metadata()
            .clone();
        received.insert(PLENORA_FIELD_ID_KEY.to_owned(), "7".to_owned());
        let field = Field::new("geom", DataType::Binary, true).with_metadata(received);
        let keys = read_geometry_contract_keys(&field).expect("read con field_id");
        assert_eq!(keys.field_id, Some(FieldId(7)));

        // Round-trip PROJJSON: la definizione sopravvive byte-per-byte.
        let projjson = r#"{"type":"ProjectedCRS","name":"demo"}"#;
        let field = Field::new("geom", DataType::Binary, true).with_metadata(
            canonical_geometry_metadata(
                &GeometryColumnContract {
                    crs: ContractCrs::Resolved(resolved_crs(projjson)),
                    ..full_contract()
                },
                &full_details(),
            ),
        );
        let keys = read_geometry_contract_keys(&field).expect("read projjson");
        assert_eq!(keys.crs_definition.as_deref(), Some(projjson));
        assert_eq!(keys.crs_definition_format, Some(CrsDefinitionFormat::Projjson));
        assert_eq!(keys.crs_id, None);
    }

    #[test]
    fn contract_keys_reject_non_canonical_values() {
        for (key, value) in [
            (PLENORA_GEOMETRY_ENCODING_KEY, "WKB"),
            (PLENORA_GEOMETRY_ENCODING_KEY, "twkb"),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY, "2d"),
            (PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "Exact"),
            (PLENORA_GEOMETRY_SRID_KEY, "-1"),
            (PLENORA_GEOMETRY_SRID_KEY, "+7"),
            (PLENORA_GEOMETRY_SRID_KEY, "4326.0"),
            (PLENORA_GEOMETRY_SRID_KEY, " 4326"),
            (PLENORA_GEOMETRY_SRID_KEY, "99999999999999999999"),
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "RESOLVED"),
            (PLENORA_GEOMETRY_CRS_ID_KEY, ""),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "geojson"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "long_lat"),
            (PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY, "geom"),
            (PLENORA_GEOMETRY_PRECISION_KEY, "f64"),
            (PLENORA_FIELD_ID_KEY, ""),
            (PLENORA_FIELD_ID_KEY, "field#7"),
        ] {
            let field = field_with_pairs(&[(key, value)]);
            assert!(
                read_geometry_contract_keys(&field).is_err(),
                "{key} con valore non canonico deve essere rifiutato (R5.1)"
            );
        }
        // Coppia definizione/formato: una sola delle due -> errore (R2.2);
        // `projjson` che non e' un oggetto JSON -> errore (R5.1).
        for pairs in [
            &[(PLENORA_GEOMETRY_CRS_DEFINITION_KEY, "PROJCS[demo]")][..],
            &[(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt")][..],
            &[
                (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, "EPSG:4326"),
                (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
            ][..],
        ] {
            let field = field_with_pairs(pairs);
            assert!(read_geometry_contract_keys(&field).is_err());
        }
        // `crs_id` senza `axis_order` -> errore (tabella R2.2); valore
        // `unknown` esplicito -> ammesso.
        let without_axis = field_with_pairs(&[(PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326")]);
        assert!(matches!(
            read_geometry_contract_keys(&without_axis),
            Err(PlenoraError::InvalidPlan(_))
        ));
        let with_unknown_axis = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
        ]);
        assert_eq!(
            read_geometry_contract_keys(&with_unknown_axis)
                .expect("unknown ammesso")
                .axis_order,
            Some(AxisOrder::Unknown)
        );
        // `crs_resolution = missing` non ammette metadati CRS dichiarati.
        let missing_with_srid = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "missing"),
            (PLENORA_GEOMETRY_SRID_KEY, "4326"),
        ]);
        assert!(read_geometry_contract_keys(&missing_with_srid).is_err());
    }

    #[test]
    fn contract_keys_enforce_r341_coherences() {
        // `exact` senza elenco -> errore.
        let exact_without = field_with_pairs(&[(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "exact")]);
        assert!(matches!(
            read_geometry_contract_keys(&exact_without),
            Err(PlenoraError::InvalidPlan(_))
        ));
        // `unresolved` con elenco -> errore.
        let unresolved_with = field_with_pairs(&[
            (PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "unresolved"),
            (PLENORA_GEOMETRY_TYPES_KEY, "point"),
        ]);
        assert!(read_geometry_contract_keys(&unresolved_with).is_err());
        // Elenco senza dichiarazione -> errore (R3.4.1: il produttore conforme
        // emette sempre `types_declaration`).
        let types_only = field_with_pairs(&[(PLENORA_GEOMETRY_TYPES_KEY, "point")]);
        assert!(read_geometry_contract_keys(&types_only).is_err());
        // Forma canonica dell'elenco: ordine §3.1, valori unici, senza spazi.
        for list in ["polygon,point", "point,point", "point, polygon", "POINT"] {
            let field = field_with_pairs(&[
                (PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "exact"),
                (PLENORA_GEOMETRY_TYPES_KEY, list),
            ]);
            assert!(read_geometry_contract_keys(&field).is_err(), "{list}");
        }
        // `mixed` con e senza elenco -> ok; entrambe le chiavi assenti ->
        // None («proprieta' non dichiarata», NON `unresolved`).
        for pairs in [
            &[(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "mixed")][..],
            &[
                (PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "mixed"),
                (PLENORA_GEOMETRY_TYPES_KEY, "point,polygon"),
            ][..],
        ] {
            let keys = read_geometry_contract_keys(&field_with_pairs(pairs)).expect("mixed");
            assert_eq!(
                keys.types.as_ref().map(GeometryTypesProperty::declaration),
                Some(TypesDeclaration::Mixed)
            );
        }
        let bare = Field::new("geom", DataType::Binary, true);
        assert_eq!(
            read_geometry_contract_keys(&bare).expect("bare").types,
            None
        );
    }

    #[test]
    fn contract_version_gate_enforces_r25() {
        let canonical_field = field_with_pairs(&[(PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy")]);
        // Versione assente + chiavi canoniche presenti -> errore (R2.5 la
        // richiede: la versione descrive il protocollo che quelle chiavi
        // seguono, senza di essa non sono interpretabili).
        assert!(matches!(
            read_contract_version(&Schema::new(vec![canonical_field.clone()])),
            Err(PlenoraError::InvalidPlan(_))
        ));
        // Versione assente + nessuna chiave canonica -> Ok(None): input
        // legacy o non plenora, nessun protocollo da verificare.
        assert_eq!(
            read_contract_version(&Schema::new(vec![Field::new("id", DataType::Int64, false)]))
                .expect("legacy"),
            None
        );
        // Versioni 0 e 1 accettate: R2.5 impone il fallimento solo per
        // versioni successive a quella nota.
        for (version, expected) in [("0", 0), ("1", 1)] {
            let schema = Schema::new_with_metadata(
                vec![canonical_field.clone()],
                HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), version.to_owned())]),
            );
            assert_eq!(
                read_contract_version(&schema).expect("versione nota"),
                Some(expected)
            );
        }
        // Versione successiva -> errore esplicito, mai interpretazione
        // parziale.
        let future = Schema::new_with_metadata(
            vec![canonical_field.clone()],
            HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "2".to_owned())]),
        );
        assert!(matches!(
            read_contract_version(&future),
            Err(PlenoraError::Unsupported(_))
        ));
        // Valore non numerico -> errore (R5.4).
        let broken = Schema::new_with_metadata(
            vec![canonical_field],
            HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1.0".to_owned())]),
        );
        assert!(matches!(
            read_contract_version(&broken),
            Err(PlenoraError::InvalidPlan(_))
        ));
        // Helper di emissione: schema con la versione corrente.
        let emitted =
            Schema::new_with_metadata(Vec::<Field>::new(), canonical_schema_version_metadata());
        assert_eq!(
            read_contract_version(&emitted).expect("emesso"),
            Some(PLENORA_CONTRACT_VERSION)
        );
    }

    #[test]
    fn contract_keys_reject_divergent_legacy_metadata() {
        // Ogni nozione presente in DUE rappresentazioni deve coincidere
        // (R2.6: il componente fallisce, non sceglie).
        let divergent_dimensions = field_with_pairs(&[
            (PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
            (GEO_METADATA_KEY, r#"{"crs":"EPSG:3857","dimensions":"xyz"}"#),
        ]);
        assert!(matches!(
            read_geometry_contract_keys(&divergent_dimensions),
            Err(PlenoraError::InvalidPlan(_))
        ));
        let divergent_encoding = field_with_pairs(&[
            (PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
            (GEO_METADATA_KEY, r#"{"crs":"EPSG:3857","encoding":"ewkb"}"#),
        ]);
        assert!(read_geometry_contract_keys(&divergent_encoding).is_err());
        let divergent_crs = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            (GEO_METADATA_KEY, r#"{"crs":"EPSG:3857"}"#),
        ]);
        assert!(read_geometry_contract_keys(&divergent_crs).is_err());
        // Canonico `missing` + legacy che dichiara un CRS -> divergenza.
        let missing_with_legacy_crs = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, "missing"),
            (GEO_METADATA_KEY, r#"{"crs":"EPSG:3857"}"#),
        ]);
        assert!(read_geometry_contract_keys(&missing_with_legacy_crs).is_err());
        // Forme non confrontabili (`crs_id` canonico vs oggetto legacy) ->
        // divergenza: mai una scelta arbitraria.
        let incomparable = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_ID_KEY, "EPSG:4326"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            (GEO_METADATA_KEY, r#"{"crs":{"type":"GeographicCRS","name":"WGS 84"}}"#),
        ]);
        assert!(read_geometry_contract_keys(&incomparable).is_err());
        // PROJJSON canonico vs oggetto legacy diverso -> divergenza.
        let divergent_projjson = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, r#"{"type":"ProjectedCRS","name":"a"}"#),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            (GEO_METADATA_KEY, r#"{"crs":{"type":"ProjectedCRS","name":"b"}}"#),
        ]);
        assert!(read_geometry_contract_keys(&divergent_projjson).is_err());
    }

    #[test]
    fn contract_keys_complete_by_precedence_never_arbitrate() {
        // Solo legacy: adottato (completamento, R2.7).
        let legacy_only = field_with_pairs(&[(
            GEO_METADATA_KEY,
            r#"{"crs":"EPSG:3857","dimensions":"xyz","encoding":"ewkb"}"#,
        )]);
        let keys = read_geometry_contract_keys(&legacy_only).expect("legacy");
        assert_eq!(keys.crs_id.as_deref(), Some("EPSG:3857"));
        assert_eq!(keys.dimensions, Some(GeometryDimensions::Xyz));
        assert_eq!(keys.encoding, Some(GeometryEncoding::Ewkb));

        // Legacy PROJJSON: adottato come definizione con formato dichiarato.
        let legacy_object = field_with_pairs(&[(
            GEO_METADATA_KEY,
            r#"{"crs":{"type":"GeographicCRS","name":"WGS 84"}}"#,
        )]);
        let keys = read_geometry_contract_keys(&legacy_object).expect("legacy object");
        assert_eq!(keys.crs_definition_format, Some(CrsDefinitionFormat::Projjson));
        let definition = keys.crs_definition.as_deref().expect("definition");
        let parsed: serde_json::Value = serde_json::from_str(definition).expect("json");
        assert_eq!(parsed["type"], "GeographicCRS");
        assert_eq!(keys.crs_id, None);

        // Entrambe presenti e coerenti: ok; l'ordine delle chiavi PROJJSON
        // diverso non conta (confronto per valore JSON, non per stringa).
        let coherent = field_with_pairs(&[
            (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, r#"{"name":"a","type":"ProjectedCRS"}"#),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
            (
                GEO_METADATA_KEY,
                r#"{"crs":{"type":"ProjectedCRS","name":"a"},"dimensions":"xy"}"#,
            ),
        ]);
        assert!(read_geometry_contract_keys(&coherent).is_ok());

        // Solo lo standard esterno: `encoding` completato da `geoarrow.wkb`.
        let extension_only = field_with_pairs(&[(GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION)]);
        let keys = read_geometry_contract_keys(&extension_only).expect("extension");
        assert_eq!(keys.encoding, Some(GeometryEncoding::Wkb));
        assert_eq!(keys.dimensions, None);

        // Canonico presente e legacy coerente: il valore e' quello canonico
        // (coincidono) e la lettura non fallisce.
        let canonical_wins = field_with_pairs(&[
            (PLENORA_GEOMETRY_ENCODING_KEY, "ewkb"),
            (GEO_METADATA_KEY, r#"{"encoding":"ewkb"}"#),
        ]);
        assert_eq!(
            read_geometry_contract_keys(&canonical_wins)
                .expect("coerente")
                .encoding,
            Some(GeometryEncoding::Ewkb)
        );

        // Campo nudo: tutto assente, mai default (R5.2).
        let bare = Field::new("geom", DataType::Binary, true);
        assert_eq!(
            read_geometry_contract_keys(&bare).expect("bare"),
            CanonicalGeometryKeys::default()
        );

        // Metadato legacy rotto: rango legacy assente, nessun errore (come i
        // reader esistenti).
        let broken = field_with_pairs(&[(GEO_METADATA_KEY, "non json")]);
        assert_eq!(
            read_geometry_contract_keys(&broken).expect("rotto"),
            CanonicalGeometryKeys::default()
        );

        // Legacy presente ma non canonico -> errore anche nel rango legacy.
        for raw in [
            r#"{"dimensions":"2d"}"#,
            r#"{"dimensions":42}"#,
            r#"{"encoding":"twkb"}"#,
            r#"{"crs":""}"#,
            r#"{"crs":42}"#,
        ] {
            let field = field_with_pairs(&[(GEO_METADATA_KEY, raw)]);
            assert!(read_geometry_contract_keys(&field).is_err(), "{raw}");
        }
    }
}
