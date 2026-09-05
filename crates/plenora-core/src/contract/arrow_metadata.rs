//! Codec dei metadati contrattuali sugli schemi Arrow.
//!
//! Legge e scrive le chiavi canoniche `plenora.*` e i metadati `GeoArrow` su
//! campi e schemi. **Non tocca una sola cella di dati**: lavora sui metadati,
//! e questa e' la ragione per cui puo' vivere qui invece che nei kernel.
//!
//! # Perche' e' in `plenora-core`
//!
//! Due componenti diversi decidono con questo codec come interpretare ed
//! emettere uno schema: la CLI in ingresso, l'executor in uscita. Se il codec
//! vivesse in `plenora-kernels-geo`, insieme alle operazioni sulle celle WKB,
//! nessuno dei due lo avrebbe sotto di se' e ciascuno ne ricaverebbe le
//! proprie conclusioni.
//!
//! Con l'esecuzione isolata in un processo worker quel confine non e' una
//! questione di ordine ma un protocollo: supervisore e
//! worker devono leggere lo stesso schema allo stesso modo, altrimenti la
//! verifica della pubblicazione confronta due interpretazioni invece di due
//! risultati. L'autorita' deve quindi stare sotto entrambi, e `plenora-core`
//! e' l'unico posto che lo e'.
//!
//! Le operazioni su celle WKB — decode, encode, stima dei byte nativi —
//! restano nei kernel geo, dove hanno bisogno di `geo` e `geozero`.

use std::collections::HashMap;

use crate::arrow::{DataType, Field, Schema};
use crate::contract::{
    AxisOrder, ContractCrs, CrsDefinitionFormat, CrsResolution, FieldId, GeometryColumnContract,
    GeometryDimensions, GeometryEncoding, GeometryPrecision, GeometryTypesProperty,
    SpatialSemantics,
};
use crate::crs::{
    authority_code_srid, definition_form, DefinitionForm, ResolvedCrs, MAX_CRS_DEFINITION_BYTES,
};
use crate::PlenoraError;

// Le cinque chiavi che il contratto dichiara gia' sono ri-esportate da qui:
// chi legge o scrive metadati ha un solo posto dove cercarle, e chi le
// raggiunge attraverso l'adapter geo continua a trovarle.
pub use crate::contract::{
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
    PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
    PLENORA_GEOMETRY_TYPES_KEY,
};

pub const GEOARROW_EXTENSION_KEY: &str = "ARROW:extension:name";
pub const GEOARROW_WKB_EXTENSION: &str = "geoarrow.wkb";
pub const GEO_METADATA_KEY: &str = "geo";
pub const DEFAULT_GEOMETRY_COLUMN: &str = "geometry";
pub const MAX_CELL_BYTES: u64 = 64 * 1024 * 1024;

/// Chiave canonica del framing binario delle celle (R2.1/R2.2, tabella §2:
/// `wkb` | `ewkb`).
///
/// Chiave canonica dell'elenco dei tipi (R2.2/R3.4.1: valori unici in ordine
/// §3.1 separati da `,` senza spazi; obbligatoria e non vuota se
/// `types_declaration = exact`).
///
/// Chiave canonica dello SRID (R2.2: intero decimale senza segno; opzionale,
/// emessa solo se noto).
pub const PLENORA_GEOMETRY_SRID_KEY: &str = "plenora.geometry.srid";
/// Chiave canonica dell'identificatore di autorita' del CRS (R2.2: es.
/// `EPSG:4326`; opzionale).
pub const PLENORA_GEOMETRY_CRS_ID_KEY: &str = "plenora.geometry.crs_id";
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
/// Scelta dichiarata: il bound NON e' reso stride-aware. Con Z/M lo
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

/// Il campo si dichiara colonna geometria WKB?
///
/// Identificazione ammessa (piano-v5.md#contratti-di-input, decisione 8): l'estensione
/// `GeoArrow` `ARROW:extension:name = geoarrow.wkb` OPPURE la forma a sole
/// chiavi canoniche — almeno una chiave `plenora.geometry.*`, autosufficiente
/// come in discovery (`plenora.geometry.encoding` +
/// `plenora.geometry.dimensions` bastano: l'estensione e' ammessa, non
/// richiesta). Un nome di estensione DIVERSO da `geoarrow.wkb` dichiara un
/// altro framing: mai accettato, anche in presenza di chiavi canoniche.
#[must_use]
pub fn field_declares_wkb_geometry(field: &Field) -> bool {
    field.metadata().get(GEOARROW_EXTENSION_KEY).map_or_else(
        || {
            field
                .metadata()
                .keys()
                .any(|key| key.starts_with(PLENORA_GEOMETRY_NAMESPACE_PREFIX))
        },
        |extension| extension == GEOARROW_WKB_EXTENSION,
    )
}

/// Errore di schema: la colonna geometria non e' `Binary`.
pub fn geometry_column_not_binary(name: &str, actual: impl std::fmt::Display) -> PlenoraError {
    PlenoraError::Schema(format!(
        "colonna geometria `{name}` di tipo {actual}, atteso Binary"
    ))
}

/// Indice della colonna geometria: deve esistere, essere `Binary` e
/// identificarsi come geometria WKB (estensione `geoarrow.wkb` o sole
/// chiavi canoniche — [`field_declares_wkb_geometry`]).
///
/// # Errors
///
/// `PlenoraError::Schema` se la colonna `name` e' assente, non e' di tipo
/// `Binary` o non si identifica come geometria WKB.
pub fn geometry_column_index(schema: &Schema, name: &str) -> Result<usize, PlenoraError> {
    let (index, field) = schema
        .column_with_name(name)
        .ok_or_else(|| missing_geometry_column(name))?;
    if field.data_type() != &DataType::Binary {
        return Err(geometry_column_not_binary(name, field.data_type()));
    }
    if !field_declares_wkb_geometry(field) {
        return Err(missing_geoarrow_metadata(name));
    }
    Ok(index)
}

/// Metadato `GeoArrow` `geo` con la chiave `crs`: PROJJSON se la definizione e'
/// gia' un oggetto JSON, altrimenti la forma authority:code come stringa.
///
/// Casa unica del formato: anche il trasporto Arrow v3 di
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
/// La dimensionalita' qui viene solo DICHIARATA nei metadati: nessun
/// percorso la propaga dai dati, e il valore e' quello che il chiamante
/// passa.
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
/// [`geo_metadata_json_with_dimensions`]: fingerprint e retrocompatibilita'
/// restano invariati per chi non dichiara l'encoding.
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
/// La dimensionalita' scritta e' sempre `Xy`, perche' i costruttori che
/// passano di qui producono WKB 2D. Non e' la dimensionalita' letta dai
/// dati: nessun percorso la propaga.
///
/// # Errors
///
/// Come [`geometry_output_field_with_encoding`].
pub fn geometry_output_field(name: &str, crs: &str) -> Result<Field, PlenoraError> {
    geometry_output_field_with_dimensions(name, crs, GeometryDimensions::Xy)
}

/// Come [`geometry_output_field`], con la dimensionalita' dichiarata dal
/// chiamante invece che fissata a `Xy`.
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
/// Un contratto con encoding dichiarato che attraversa un kernel che
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
/// Lettura opzionale e lenient di proposito: chiave assente, JSON non valido
/// o valore non riconosciuto → [`GeometryDimensions::Unknown`] (R3.4: MAI un
/// default silenzioso `Xy`). Se un valore non riconosciuto sia un errore lo
/// decide la discovery, non questa lettura.
#[must_use]
pub fn geometry_dimensions_from_metadata(field: &Field) -> GeometryDimensions {
    geo_metadata_value_lenient(field)
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
/// Lettura opzionale e lenient di proposito: chiave assente, JSON non valido
/// o valore non riconosciuto → `None` (R3.4/R3.5: MAI un default silenzioso;
/// R3.5: valori fuori dall'enum chiuso non sono rappresentabili).
#[must_use]
pub fn geometry_encoding_from_metadata(field: &Field) -> Option<GeometryEncoding> {
    geo_metadata_value_lenient(field).and_then(|value| {
        value
            .get("encoding")
            .and_then(serde_json::Value::as_str)
            .and_then(|encoding| encoding.parse().ok())
    })
}

/// Variante STRICT di [`geometry_encoding_from_metadata`], per la discovery.
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
    let Some(value) = geo_metadata_value(field)? else {
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

/// Il metadato legacy `geo` di un campo come valore JSON.
///
/// `Ok(None)` = chiave ASSENTE. JSON malformato = `Err`.
///
/// La distinzione e' il punto: piano-v5.md#contratti-di-input (R5.1) impone che «illeggibile» non
/// equivalga ad «assente». Ridurre l'errore con un `.ok()` renderebbe un
/// metadato `geo` malformato indistinguibile da uno mancante, e la
/// risoluzione del contratto proseguirebbe completando le nozioni dalle sole
/// chiavi canoniche — ignorando in silenzio un legacy coesistente che non e'
/// riuscita a leggere. Un input corrotto sarebbe cosi' accettato come se
/// dichiarasse solo cio' che si e' capito di lui.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave e' presente ma non contiene JSON
/// valido. Il messaggio non riporta il valore («errori senza dati»).
fn geo_metadata_value(field: &Field) -> Result<Option<serde_json::Value>, PlenoraError> {
    let Some(raw) = field.metadata().get(GEO_METADATA_KEY) else {
        return Ok(None);
    };
    // Metadato contrattuale: le chiavi duplicate lo rendono ambiguo e vanno
    // rifiutate, non risolte con «vince l'ultima».
    crate::json::ensure_no_duplicate_keys(raw).map_err(|_| {
        PlenoraError::InvalidPlan(
            "metadato legacy `geo`: chiavi duplicate, documento ambiguo".to_owned(),
        )
    })?;
    serde_json::from_str::<serde_json::Value>(raw)
        .map(Some)
        .map_err(|_| {
            PlenoraError::InvalidPlan(
                "metadato legacy `geo`: JSON non valido (R5.1: illeggibile non e' assente)"
                    .to_owned(),
            )
        })
}

/// Lettura opportunistica del metadato `geo`, per i soli lettori che
/// dichiarano di NON decidere ([`geometry_dimensions_from_metadata`],
/// [`geometry_encoding_from_metadata`]): un metadato illeggibile vale come
/// «nozione non dichiarata».
///
/// E' ammesso solo qui perche' quei lettori non costruiscono contratti e non
/// applicano precedenze: alimentano l'analisi a secco, dove una nozione non
/// risolta e' un esito legittimo. Ogni percorso che costruisce o confronta un
/// contratto usa la forma fallibile.
fn geo_metadata_value_lenient(field: &Field) -> Option<serde_json::Value> {
    geo_metadata_value(field).ok().flatten()
}

// ---------------------------------------------------------------------------
// Protocollo delle chiavi canoniche (contratti trasversali
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
    /// Ordine degli assi del CRS; la chiave e' obbligatoria quando un CRS
    /// e' presente e l'emissione e' per completamento DELL'ASSENTE (R2.7,
    /// piano-v5.md#contratti-di-input emendamento 2026-07-31): questo dettaglio esplicito vince,
    /// poi l'ordine GIS normalizzato quando la definizione canonica permette
    /// di stabilire gli assi. La chiave descrive l'ordine fisico x/y dei
    /// byte, non l'ordine nativo dell'autorita'.
    pub axis_order: Option<AxisOrder>,
    /// SRID noto (emesso come intero decimale senza segno); come sopra, un
    /// dettaglio assente e' completato dalla deduzione d'autorita'
    /// ([`ResolvedCrs::authority_srid`], o dalla forma `authority:code` nel
    /// percorso legacy) e resta assente solo se neanche quella decide.
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
///   modellato dal contratto: resta propagato dalla lineage). Con la SOLA
///   rappresentazione SRID (R4.3.1: il produttore conosce il codice
///   numerico, non l'autorita' — R4.4) il blocco non emette nemmeno
///   `axis_order` (la tabella R2.2 lo impone solo con `crs_id` o
///   `crs_definition`): sintetizzarlo sarebbe una dichiarazione inventata;
/// - con `missing` NON sono emesse `crs_id`/`crs_definition`/
///   `crs_definition_format`/`axis_order`/`srid` (coerenza R2.2:
///   `crs_resolution = missing` non ammette metadati CRS dichiarati);
/// - la forma della definizione CRS decide la chiave
///   ([`definition_form`], piano-v5.md#contratti-di-input emendamento 2026-07-31): un oggetto
///   JSON (PROJJSON) e' emesso come `crs_definition` + `projjson`, un testo
///   WKT1/WKT2 come `crs_definition` + `wkt`/`wkt2`, un identificatore di
///   autorita' (es. `EPSG:4326`) come `crs_id`. E' la stessa distinzione che
///   [`geo_metadata_json`] applica al metadato legacy `geo.crs` (oggetto
///   JSON incorporato vs stringa authority:code), cosi' le due
///   rappresentazioni sono coerenti per costruzione (R2.6); e' anche la
///   forma emessa da plenora-database-tools (`crs_id` = `EPSG:xxxx`).
///   Limite preesistente invariato: una proj-string (`+proj=...`) non ha
///   formato nella tabella §2 e resta in `crs_id`
///   ([`DefinitionForm::Other`]).
/// - con un CRS risolto `axis_order` e' sempre emesso, e con `declared_unresolved`
///   e' emesso quando `crs_id` o `crs_definition` e' presente (obbligatorio
///   in quei casi) per completamento DELL'ASSENTE, mai arbitrato (R2.7 —
///   piano-v5.md#contratti-di-input, emendamento 2026-07-31): vince il dettaglio esplicito, poi
///   l'ordine GIS normalizzato quando la definizione canonica permette di
///   stabilire gli assi, cioe' l'ordine fisico x/y letto e scritto dai
///   kernel; `unknown` resta il fallback quando gli assi non sono deducibili (la
///   chiave qui e' obbligatoria, non opzionale, e `unknown` non e' un
///   default al posto dell'assente — R5.2 riguarda le chiavi opzionali:
///   `srid`, `spatial_semantics`, `precision`, `encoding`. `srid` segue la
///   stessa cascata di completamento via
///   [`ResolvedCrs::authority_srid`] e resta assente se neanche la
///   deduzione decide).
/// - `types`/`types_declaration` sono emesse SOLO se il campo `types` porta
///   un valore (confidence `Declared`/`Proven`/`Estimated`); confidence
///   `Unknown` («proprieta' non dichiarata», R3.4.1) non emette nulla: mai
///   inventare una dichiarazione. `types` e' omessa quando l'elenco e' vuoto
///   (`unresolved`, o `mixed` senza elenco), come da forma canonica.
///
/// Le chiavi `GeoArrow` (`ARROW:extension:name`, `geo`) RESTANO emesse dai
/// costruttori esistenti (R2.6 ammette la coesistenza se coerente): questa
/// funzione produce solo il blocco canonico; la fusione nei campi di output
/// e' responsabilita' del chiamante, cosi' come
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
            insert_resolved_crs_keys(&mut metadata, crs.definition(), details, Some(crs));
        }
        ContractCrs::DeclaredUnresolved {
            crs_id,
            definition,
            definition_format,
        } => {
            // R4.6.4: le dichiarazioni originali sono ri-emesse invariate —
            // l'incoerenza arriva al bordo di scrittura com'e', mai persa
            // e mai conciliata. `axis_order` e' emesso come per `resolved`
            // (obbligatorio per la tabella R2.2 quando `crs_id` o
            // `crs_definition` e' presente): qui NON c'e' un `ResolvedCrs`
            // da cui dedurre (lo stato porta le dichiarazioni, non una
            // definizione risolta), quindi senza dettaglio esplicito vale
            // `unknown` — l'assenza di una dichiarazione, che non
            // sovrascrive la lineage (vedi `canonical_output_schema`).
            // Con la SOLA rappresentazione SRID (R4.3.1 — il produttore
            // conosce il codice, non l'autorita') la tabella R2.2 non
            // impone `axis_order` e il centro non lo sintetizza (R4.4):
            // resta alla lineage, se dichiarato.
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
            if crs_id.is_some() || definition.is_some() {
                let axis_order = details.axis_order.unwrap_or(AxisOrder::Unknown);
                metadata.insert(
                    PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                    axis_order.as_str().to_owned(),
                );
            }
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
    // grafo che lo ha assegnato (piano-v5.md#contratti-di-input, decisione 3) — non ha
    // significato fuori dal processo. Una chiave `plenora.field_id`
    // RICEVUTA resta propagata invariata dalla lineage (R2.4), mai
    // sovrascritta dal valore di grafo.
    metadata
}

/// Chiavi CRS di uno stato `resolved` (R2.2): corpo condiviso fra
/// [`canonical_geometry_metadata`] (braccio `Resolved`/`ResolvedByDecision`,
/// che passa il `ResolvedCrs`) e
/// [`canonical_geometry_metadata_for_resolved_definition`] (trasporto
/// legacy, che passa `None` e deduce lo `srid` a monte dalla forma
/// `authority:code`) — stessa forma e stessi byte a parita' di definizione
/// e dettagli.
///
/// La FORMA della definizione decide la chiave ([`definition_form`],
/// piano-v5.md#contratti-di-input emendamento 2026-07-31 — classe B): oggetto JSON (PROJJSON) →
/// `crs_definition` + `crs_definition_format = projjson`; testo WKT1/WKT2
/// → `crs_definition` (byte originali) + `wkt`/`wkt2` (etichetta derivata
/// dalla stringa stessa: passthrough idempotente contro la lineage, nessuno
/// stato nuovo in `ResolvedCrs`); identificatore d'autorita' e ogni altra
/// forma → `crs_id`. E' la stessa distinzione che [`geo_metadata_json`]
/// applica al metadato legacy `geo.crs` (oggetto JSON incorporato vs
/// stringa authority:code), cosi' le due rappresentazioni restano coerenti
/// per costruzione (R2.6). Mandare ogni testo non-JSON in `crs_id` sarebbe
/// sbagliato per il WKT: romperebbe il passthrough R2.6 contro una lineage
/// `crs_definition = wkt`. Limite dichiarato: una proj-string non ha formato
/// nella tabella §2
/// e resta in `crs_id` ([`DefinitionForm::Other`]).
///
/// `axis_order` e' sempre emesso (obbligatorio quando un CRS e' presente)
/// con completamento DELL'ASSENTE, mai arbitrato (R2.7 — piano-v5.md#contratti-di-input,
/// emendamento 2026-07-31): vince il dettaglio esplicito di `details`, poi
/// l'ordine GIS normalizzato quando la definizione canonica permette di
/// stabilire gli assi, cioe' l'ordine fisico x/y letto e scritto dai kernel;
/// `unknown` resta il fallback quando gli assi non sono deducibili (non un
/// default al posto dell'assente: la chiave qui e' obbligatoria). `srid`
/// (opzionale R5.2) segue la stessa cascata senza fondo: dettaglio
/// esplicito, poi deduzione ([`ResolvedCrs::authority_srid`]), altrimenti
/// chiave assente.
fn insert_resolved_crs_keys(
    metadata: &mut HashMap<String, String>,
    definition: &str,
    details: &GeometryMetadataDetails,
    resolved: Option<&ResolvedCrs>,
) {
    let (key, definition_format) = match definition_form(definition) {
        DefinitionForm::Projjson => (
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            Some(CrsDefinitionFormat::Projjson),
        ),
        DefinitionForm::Wkt => (
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            Some(CrsDefinitionFormat::Wkt),
        ),
        DefinitionForm::Wkt2 => (
            PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
            Some(CrsDefinitionFormat::Wkt2),
        ),
        DefinitionForm::AuthorityCode | DefinitionForm::Other => {
            (PLENORA_GEOMETRY_CRS_ID_KEY, None)
        }
    };
    metadata.insert(key.to_owned(), definition.to_owned());
    if let Some(format) = definition_format {
        metadata.insert(
            PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY.to_owned(),
            format.as_str().to_owned(),
        );
    }
    let axis_order = details
        .axis_order
        .or_else(|| {
            resolved.and_then(|crs| {
                crs.authority_axis_order()
                    .map(|_| crs.normalized_gis_axis_order())
            })
        })
        .unwrap_or(AxisOrder::Unknown);
    metadata.insert(
        PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
        axis_order.as_str().to_owned(),
    );
    if let Some(srid) = details
        .srid
        .or_else(|| resolved.and_then(ResolvedCrs::authority_srid))
    {
        metadata.insert(PLENORA_GEOMETRY_SRID_KEY.to_owned(), srid.to_string());
    }
}

/// Blocco canonico R2.2 da una definizione CRS gia' risolta al bordo del
/// produttore, senza un [`ResolvedCrs`].
///
/// BLOCK-06 (parita' del percorso legacy col v4,
/// errori-e-limiti.md#limiti-dichiarati estesa): il trasporto legacy `geo_transport` valida il CRS
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
///
/// Deduzione d'autorita' (piano-v5.md#contratti-di-input, emendamento 2026-07-31): senza un
/// `ResolvedCrs` la definizione canonica non e' disponibile, quindi lo
/// `srid` e' dedotto dalla forma `authority:code` della definizione quando
/// numerica ([`authority_code_srid`] — `EPSG:4326` → 4326), mentre
/// `axis_order` resta `unknown` — LIMITE DICHIARATO: il trasporto legacy
/// non risolve la definizione e non puo' dedurre gli assi onestamente
/// (dedurli dalla stringa sarebbe inventarli).
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
    // Legacy: nessun `ResolvedCrs` da passare — lo `srid` e' dedotto qui
    // dalla forma `authority:code` (completamento dell'assente: un
    // `details.srid` esplicito vince); `axis_order` resta `unknown` nel
    // corpo condiviso (limite dichiarato nel doc sopra).
    let effective_details = &GeometryMetadataDetails {
        srid: details.srid.or_else(|| authority_code_srid(definition)),
        ..*details
    };
    insert_resolved_crs_keys(&mut metadata, definition, effective_details, None);
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

/// Chiavi canoniche dei tipi geometrici riscritte dalle trasformazioni che
/// CAMBIANO il tipo della colonna (piano-v5.md#contratti-di-input, decisione 8).
///
/// Il contratto di output prodotto dall'analisi dichiara i tipi
/// dell'output; le chiavi ereditate dal campo di input descriverebbero il
/// fatto prima della trasformazione.
pub const TYPES_KEYS_REWRITTEN_BY_TRANSFORM: [&str; 2] = [
    PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
    PLENORA_GEOMETRY_TYPES_KEY,
];

/// Rimuove le chiavi canoniche dei tipi dai metadati di un campo geometria.
///
/// Usata dall'analisi delle trasformazioni che cambiano il tipo geometrico
/// (piano-v5.md#contratti-di-input, decisione 8): il contratto dichiara i tipi dell'output e il
/// blocco canonico li ri-emette da li' — la dichiarazione ereditata non
/// deve sopravvivere accanto (un consumatore a valle leggerebbe il tipo di
/// prima della trasformazione) ne' provocare un conflitto R2.6.
pub fn strip_rewritten_types_declarations<S: std::hash::BuildHasher>(
    metadata: &mut HashMap<String, String, S>,
) {
    for key in TYPES_KEYS_REWRITTEN_BY_TRANSFORM {
        metadata.remove(key);
    }
}

/// Rimuove le chiavi canoniche del CRS dai metadati di un campo geometria,
/// SENZA toccare il metadato legacy `geo` (gia' riscritto dall'operazione
/// col CRS di output).
///
/// Usata dall'analisi di `geo.reproject` (piano-v5.md#contratti-di-input, decisione 8): la
/// riproiezione CAMBIA il fatto (il CRS della colonna), non ne descrive uno
/// diverso — le chiavi della sorgente sono sostituite e il blocco canonico
/// ri-emette il target dal contratto, senza conflitto R2.6 con la lineage.
pub fn strip_rewritten_crs_keys<S: std::hash::BuildHasher>(
    metadata: &mut HashMap<String, String, S>,
) {
    for key in CRS_KEYS_REPLACED_BY_DECISION {
        metadata.remove(key);
    }
}

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
pub fn canonical_geometry_encoding(
    field: &Field,
) -> Result<Option<GeometryEncoding>, PlenoraError> {
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
    let declaration = parse_canonical_enum::<crate::contract::TypesDeclaration>(
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
/// limiti testuali di [`MAX_CRS_DEFINITION_BYTES`] e la sua forma deve
/// corrispondere al formato dichiarato (R5.1: una definizione incoerente non
/// viene mai reinterpretata).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se una sola delle due chiavi e' presente, se il
/// formato non e' canonico, se la definizione e' vuota/oltre il limite o se
/// il contenuto non corrisponde al formato dichiarato.
pub fn canonical_geometry_crs_definition(
    field: &Field,
) -> Result<Option<(String, CrsDefinitionFormat)>, PlenoraError> {
    let definition = field.metadata().get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY);
    let format = parse_canonical_enum::<CrsDefinitionFormat>(
        field
            .metadata()
            .get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY),
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
            let actual_format = match definition_form(definition) {
                DefinitionForm::Projjson => Some(CrsDefinitionFormat::Projjson),
                DefinitionForm::Wkt => Some(CrsDefinitionFormat::Wkt),
                DefinitionForm::Wkt2 => Some(CrsDefinitionFormat::Wkt2),
                DefinitionForm::AuthorityCode | DefinitionForm::Other => None,
            };
            if actual_format != Some(format) {
                return Err(PlenoraError::InvalidPlan(format!(
                    "chiave `{PLENORA_GEOMETRY_CRS_DEFINITION_KEY}`: il contenuto non \
                     corrisponde al formato `{format}` dichiarato (R5.1)"
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
    Ok(parse_canonical_u32(
        field.metadata().get(PLENORA_FIELD_ID_KEY),
        PLENORA_FIELD_ID_KEY,
    )?
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

/// CRS letto dal metadato legacy `geo`: identificatore testuale, definizione
/// WKT/WKT2 testuale oppure oggetto PROJJSON incorporato.
enum LegacyCrs {
    Id(String),
    Definition(serde_json::Value),
    TextDefinition {
        text: String,
        format: CrsDefinitionFormat,
    },
}

/// Le nozioni trasportate dal metadato legacy `geo` (crs, dimensions,
/// encoding), lette in forma STRICT.
struct LegacyGeoKeys {
    crs: Option<LegacyCrs>,
    dimensions: Option<GeometryDimensions>,
    encoding: Option<GeometryEncoding>,
}

/// Lettura STRICT del metadato legacy `geo`: chiave assente → metadato
/// assente; JSON non valido o valore non canonico → errore esplicito (R5.1
/// applicato al rango legacy nella lettura di contratto — «illeggibile» non
/// e' «assente», e un legacy malformato non va scavalcato dalle chiavi
/// canoniche nel completamento per precedenza R2.7).
fn legacy_geo_keys(field: &Field) -> Result<LegacyGeoKeys, PlenoraError> {
    let encoding = geometry_encoding_from_metadata_strict(field)?;
    let Some(value) = geo_metadata_value(field)? else {
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
            match definition_form(text) {
                DefinitionForm::Wkt => Some(LegacyCrs::TextDefinition {
                    text: text.clone(),
                    format: CrsDefinitionFormat::Wkt,
                }),
                DefinitionForm::Wkt2 => Some(LegacyCrs::TextDefinition {
                    text: text.clone(),
                    format: CrsDefinitionFormat::Wkt2,
                }),
                DefinitionForm::Projjson
                | DefinitionForm::AuthorityCode
                | DefinitionForm::Other => Some(LegacyCrs::Id(text.clone())),
            }
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

fn legacy_crs_is_coherent(keys: &CanonicalGeometryKeys, legacy: &LegacyCrs) -> bool {
    match legacy {
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
        LegacyCrs::TextDefinition { text, format } => {
            keys.crs_definition_format == Some(*format)
                && keys.crs_definition.as_ref() == Some(text)
        }
    }
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
    let (crs_definition, crs_definition_format) = match canonical_geometry_crs_definition(field)? {
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
        && field
            .metadata()
            .get(GEOARROW_EXTENSION_KEY)
            .map(String::as_str)
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
                LegacyCrs::TextDefinition { text, format } => {
                    keys.crs_definition = Some(text);
                    keys.crs_definition_format = Some(format);
                }
            }
        } else if !legacy_crs_is_coherent(&keys, &legacy_crs) {
            return Err(divergent_geometry_keys("crs"));
        }
    }

    Ok(keys)
}
