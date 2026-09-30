//! Codec dei metadati contrattuali sugli schemi Arrow.
//!
//! Legge e scrive le chiavi canoniche `plenora.*` e i metadati `GeoArrow` su
//! campi e schemi, senza toccare una cella di dati. Sta in `plenora-core`
//! perché kernel geo, analisi dei contratti e `plenora-io` devono
//! interpretare ed emettere uno schema allo stesso modo: l'autorità sta
//! sotto tutti. Le operazioni sulle celle WKB restano nei kernel geo.
//!
//! Le chiavi canoniche `plenora.geometry.*` seguono il contratto
//! d'interfaccia del progetto d'origine: lettura fail-closed per chiave
//! («illeggibile» non è «assente»), una chiave presente sia in forma
//! canonica sia nel metadato `geo` deve coincidere (il componente fallisce,
//! non sceglie), e le nozioni assenti si completano per precedenza
//! (canonica, poi `geo`, poi l'estensione `geoarrow.wkb`), mai con un
//! default.

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

// Le chiavi che il contratto dichiara gia' sono ri-esportate da qui: chi
// legge o scrive metadati ha un solo posto dove cercarle.
pub use crate::contract::{
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_DIMENSIONS_KEY,
    PLENORA_GEOMETRY_ENCODING_KEY, PLENORA_GEOMETRY_TYPES_DECLARATION_KEY,
    PLENORA_GEOMETRY_TYPES_KEY,
};

/// Chiave di metadato del campo con il nome dell'estensione Arrow.
pub const GEOARROW_EXTENSION_KEY: &str = "ARROW:extension:name";
/// Nome dell'estensione `GeoArrow` delle colonne WKB, l'unica accettata.
pub const GEOARROW_WKB_EXTENSION: &str = "geoarrow.wkb";
/// Chiave del metadato `GeoArrow` `geo` del campo (JSON con `crs`,
/// `dimensions`, `encoding`).
pub const GEO_METADATA_KEY: &str = "geo";
/// Nome della colonna geometria prodotta dai produttori geo (`from_coords`,
/// `from_wkt`, `generate_grid`...).
pub const DEFAULT_GEOMETRY_COLUMN: &str = "geometry";
/// Byte massimi di una cella WKB, 64 MiB.
pub const MAX_CELL_BYTES: u64 = 64 * 1024 * 1024;

/// Chiave canonica dello SRID (intero decimale senza segno; opzionale,
/// emessa solo se noto).
pub const PLENORA_GEOMETRY_SRID_KEY: &str = "plenora.geometry.srid";
/// Chiave canonica dell'identificatore di autorità del CRS (es.
/// `EPSG:4326`; opzionale).
pub const PLENORA_GEOMETRY_CRS_ID_KEY: &str = "plenora.geometry.crs_id";
/// Chiave canonica della definizione CRS testuale (WKT o PROJJSON;
/// opzionale, richiede `crs_definition_format`).
pub const PLENORA_GEOMETRY_CRS_DEFINITION_KEY: &str = "plenora.geometry.crs_definition";
/// Chiave canonica del formato della definizione CRS (`wkt` | `wkt2` |
/// `projjson`; obbligatoria se `crs_definition` è presente).
pub const PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY: &str =
    "plenora.geometry.crs_definition_format";
/// Chiave canonica dell'ordine degli assi (`lon_lat` | `lat_lon` |
/// `easting_northing` | `northing_easting` | `other` | `unknown`;
/// obbligatoria se `crs_id` o `crs_definition` è presente).
pub const PLENORA_GEOMETRY_AXIS_ORDER_KEY: &str = "plenora.geometry.axis_order";
/// Chiave canonica della semantica spaziale (`geometry` | `geography`;
/// opzionale).
pub const PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY: &str = "plenora.geometry.spatial_semantics";
/// Chiave canonica della precisione delle coordinate (`float64` |
/// `float32` | `native`; opzionale).
pub const PLENORA_GEOMETRY_PRECISION_KEY: &str = "plenora.geometry.precision";
/// Chiave canonica dell'identità logica stabile della colonna (intero
/// decimale senza segno; opzionale).
pub const PLENORA_FIELD_ID_KEY: &str = "plenora.field_id";
/// Chiave di versione del contratto dei metadati (intero decimale; vive in
/// `Schema::metadata`, MAI nel campo, ed è obbligatoria se sono presenti
/// chiavi canoniche).
pub const PLENORA_CONTRACT_VERSION_KEY: &str = "plenora.contract.version";
/// Versione corrente del contratto dei metadati: un consumatore che riceve
/// una versione maggiore DEVE fallire in modo esplicito, mai interpretare
/// parzialmente.
pub const PLENORA_CONTRACT_VERSION: u32 = 1;

/// Prefisso del namespace canonico: usato per rilevare la presenza di
/// chiavi canoniche, che rende obbligatoria la versione del contratto.
const PLENORA_NAMESPACE_PREFIX: &str = "plenora.";
/// Prefisso del namespace geometrico canonico (`plenora.geometry.*`).
///
/// Un campo che porta almeno una di queste chiavi si dichiara colonna
/// geometrica in forma autosufficiente, anche in assenza dei metadati
/// `GeoArrow`. Resta escluso `plenora.field_id`, che non è specifico delle
/// geometrie.
pub const PLENORA_GEOMETRY_NAMESPACE_PREFIX: &str = "plenora.geometry.";
/// Lunghezza massima dell'identificatore di autorita' `crs_id` (allineata al
/// lettore di plenora-database-tools, stessa regola di robustezza).
const MAX_CRS_ID_BYTES: usize = 1_024;

/// Coordinate massime per cella, a 16 byte per coordinata XY.
///
/// Il limite non tiene conto dello stride Z/M (24/32 byte): resta permissivo
/// ma mai sotto il reale, e irrigidirlo richiederebbe una dimensionalità
/// risolta che per `Unknown` non esiste.
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
/// Vale l'estensione `geoarrow.wkb` oppure la sola presenza di chiavi
/// `plenora.geometry.*`. Un nome di estensione diverso da `geoarrow.wkb`
/// dichiara un altro framing e non è mai accettato, anche con chiavi
/// canoniche.
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
/// Casa unica del formato: chi scrive il metadato `geo` passa di qui, così
/// il JSON è identico byte per byte in ogni percorso.
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

/// Come [`geo_metadata_json`], con in più la chiave `dimensions` in forma
/// canonica ([`GeometryDimensions::as_str`]).
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
/// `encoding` in forma canonica ([`GeometryEncoding::as_str`]) quando il
/// contratto la dichiara (`Some`).
///
/// Con `None` il JSON è identico byte per byte a
/// [`geo_metadata_json_with_dimensions`].
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
/// Un kernel che riscrive il campo (es. `reproject`) conserva cosi' la
/// chiave. Con `None` il metadato e' identico byte per byte alla forma senza
/// encoding.
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
/// o valore non riconosciuto → [`GeometryDimensions::Unknown`] (MAI un
/// default silenzioso `Xy`). Se un valore non riconosciuto sia un errore lo
/// decide la lettura del contratto, non questa.
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
/// o valore non riconosciuto → `None` (MAI un default silenzioso: valori
/// fuori dall'enum chiuso non sono rappresentabili).
#[must_use]
pub fn geometry_encoding_from_metadata(field: &Field) -> Option<GeometryEncoding> {
    geo_metadata_value_lenient(field).and_then(|value| {
        value
            .get("encoding")
            .and_then(serde_json::Value::as_str)
            .and_then(|encoding| encoding.parse().ok())
    })
}

/// Variante STRICT di [`geometry_encoding_from_metadata`], per la lettura
/// del contratto.
///
/// Una chiave `encoding` fuori dall'enum chiuso è un framing non
/// rappresentabile e si rifiuta, mai mappata o ignorata. Metadato `geo`
/// assente o senza chiave `encoding` danno `Ok(None)`.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se il metadato `geo` non è JSON valido o ha
/// chiavi ripetute; `PlenoraError::Unsupported` se la chiave `encoding` è
/// presente ma non rappresentabile (valore non testuale o fuori dall'enum
/// chiuso).
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
                 (ammessi solo `wkb` ed `ewkb`)"
                    .to_owned(),
            ))
        },
        |encoding| Ok(Some(encoding)),
    )
}

/// Il metadato legacy `geo` di un campo come valore JSON.
///
/// `Ok(None)` = chiave assente; JSON malformato = `Err`: «illeggibile» non è
/// «assente». Con un `.ok()` un `geo` malformato sparirebbe e il contratto
/// si completerebbe dalle sole chiavi canoniche, ignorando un metadato che
/// non si è letto.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave è presente ma non contiene JSON
/// valido o ha chiavi ripetute. Il messaggio non riporta il valore («errori
/// senza dati»).
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
                "metadato legacy `geo`: JSON non valido (illeggibile non vale assente)".to_owned(),
            )
        })
}

/// Lettura opportunistica del metadato `geo`: un metadato illeggibile vale
/// come «nozione non dichiarata».
///
/// Ammessa solo per [`geometry_dimensions_from_metadata`] e
/// [`geometry_encoding_from_metadata`], che alimentano l'analisi a secco e non
/// costruiscono ne' confrontano contratti; quei percorsi usano la forma
/// fallibile.
fn geo_metadata_value_lenient(field: &Field) -> Option<serde_json::Value> {
    geo_metadata_value(field).ok().flatten()
}

// ---------------------------------------------------------------------------
// Chiavi canoniche: emissione da `GeometryColumnContract`, lettura
// fail-closed per chiave, coerenza canonica-vs-`geo` e completamento per
// precedenza.
// ---------------------------------------------------------------------------

/// Chiavi canoniche che un [`GeometryColumnContract`] NON modella.
///
/// Chiavi opzionali che il produttore dichiara solo se note (assenti
/// restano assenti, mai un default al posto dell'assente).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryMetadataDetails {
    /// Ordine degli assi del CRS; la chiave è obbligatoria quando un CRS è
    /// presente e l'emissione completa solo L'ASSENTE: questo dettaglio
    /// esplicito vince, poi l'ordine GIS normalizzato quando la definizione
    /// canonica permette di stabilire gli assi. La chiave descrive l'ordine
    /// fisico x/y dei byte, non l'ordine nativo dell'autorità.
    pub axis_order: Option<AxisOrder>,
    /// SRID noto (emesso come intero decimale senza segno); come sopra, un
    /// dettaglio assente e' completato dalla deduzione d'autorita'
    /// ([`ResolvedCrs::authority_srid`], o dalla forma `authority:code`
    /// senza `ResolvedCrs`) e resta assente solo se neanche quella decide.
    pub srid: Option<u32>,
    /// Semantica spaziale della colonna.
    pub spatial_semantics: Option<SpatialSemantics>,
    /// Precisione delle coordinate.
    pub precision: Option<GeometryPrecision>,
}

/// Metadati di campo canonici `plenora.geometry.*` per una colonna
/// geometrica, costruiti da un [`GeometryColumnContract`] e dai dettagli che
/// il contratto non modella.
///
/// - `crs_resolution` riflette lo stato del contratto: `resolved`
///   (`ResolvedCrs` nasce solo da una risoluzione verificata: la tabella dei
///   CRS integrati o un risolutore del chiamante), `declared_unresolved` o
///   `missing` (mai un CRS inventato).
/// - Con `declared_unresolved` le dichiarazioni originali (`crs_id` e/o
///   `crs_definition` col formato) si ri-emettono invariate; `srid` resta
///   alla lineage. Con il solo SRID non si emette nemmeno `axis_order`:
///   nessuna regola lo impone e sintetizzarlo sarebbe inventarlo.
/// - Con `missing` non si emette alcuna chiave CRS.
/// - La forma della definizione decide la chiave ([`definition_form`]:
///   PROJJSON e WKT in `crs_definition` col formato, ogni altra forma in
///   `crs_id`), coerente con `geo.crs` di [`geo_metadata_json`] e con
///   plenora-database-tools.
/// - `axis_order` è obbligatorio con `crs_id` o `crs_definition` e si
///   completa solo se assente: dettaglio esplicito, poi ordine GIS
///   normalizzato dalla definizione, infine `unknown`. `srid` (opzionale)
///   segue la stessa cascata via [`ResolvedCrs::authority_srid`] e resta
///   assente se nessuno decide.
/// - `types`/`types_declaration` si emettono solo se il campo `types` porta
///   un valore: confidence `Unknown` non emette nulla; `types` è omessa con
///   elenco vuoto.
///
/// Le chiavi `GeoArrow` restano emesse dai costruttori; la fusione nei
/// campi di output e `plenora.contract.version`
/// ([`canonical_schema_version_metadata`]) spettano al chiamante.
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
            // Le dichiarazioni originali si ri-emettono invariate.
            // `axis_order` è obbligatorio con `crs_id` o `crs_definition`;
            // senza un `ResolvedCrs` da cui dedurre, vale `unknown` se manca
            // un dettaglio esplicito, e non sovrascrive la lineage (vedi
            // `arrow_schema_from_contract`). Con il solo SRID non si
            // sintetizza.
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
        // Con `crs_resolution = missing` nessuna chiave CRS è emessa:
        // `missing` non ammette `crs_id`/`crs_definition`/`srid`/`axis_order`.
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
    // `plenora.field_id` NON è emesso: è opzionale, e il `FieldId` del
    // contratto appartiene al namespace del piano che lo ha assegnato, senza
    // significato fuori dal processo. Una chiave `plenora.field_id` RICEVUTA
    // resta propagata invariata dalla lineage, mai sovrascritta dal valore
    // del piano.
    metadata
}

/// Chiavi CRS di uno stato `resolved`, corpo condiviso fra
/// [`canonical_geometry_metadata`] e
/// [`canonical_geometry_metadata_for_resolved_definition`]: stessi byte a
/// parita' di definizione e dettagli.
///
/// La forma della definizione decide la chiave ([`definition_form`]):
/// PROJJSON -> `crs_definition` + `projjson`; WKT1/WKT2 -> `crs_definition`
/// (byte originali) + `wkt`/`wkt2`; ogni altra forma -> `crs_id`. È la
/// distinzione di [`geo_metadata_json`] per `geo.crs`, quindi le due
/// rappresentazioni sono coerenti per costruzione. Limite dichiarato: una
/// proj-string non ha un formato canonico e resta in `crs_id`
/// ([`DefinitionForm::Other`]).
///
/// `axis_order` è sempre emesso e si completa solo se assente: dettaglio
/// esplicito, poi ordine GIS normalizzato dalla definizione, infine
/// `unknown`. `srid` (opzionale) segue la stessa cascata senza fondo:
/// dettaglio, poi [`ResolvedCrs::authority_srid`], altrimenti assente.
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

/// Blocco canonico `plenora.geometry.*` da una definizione CRS già risolta
/// dal chiamante, senza un [`ResolvedCrs`].
///
/// Nel progetto d'origine lo usava il trasporto geo, che portava la sola
/// definizione; qui lo usano solo i test dei kernel geo. Emette `resolved`
/// nella stessa forma di [`canonical_geometry_metadata`]; non emette
/// `types`/`types_declaration`, e `encoding` solo se dichiarata.
///
/// Lo `srid` si deduce dalla forma `authority:code` ([`authority_code_srid`]);
/// `axis_order` resta `unknown`, **limite dichiarato**: dedurre gli assi dalla
/// stringa sarebbe inventarli.
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
    // Nessun `ResolvedCrs` da passare: lo `srid` è dedotto qui
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

/// Metadati di schema con la versione del contratto
/// (`plenora.contract.version` vive in `Schema::metadata`, non nel campo, ed
/// è obbligatoria se lo schema porta chiavi canoniche).
#[must_use]
pub fn canonical_schema_version_metadata() -> HashMap<String, String> {
    HashMap::from([(
        PLENORA_CONTRACT_VERSION_KEY.to_owned(),
        PLENORA_CONTRACT_VERSION.to_string(),
    )])
}

/// Chiavi canoniche che una decisione CRS del piano sostituisce; le stesse
/// che `geo.reproject` riscrive ([`strip_rewritten_crs_keys`]).
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
/// CAMBIANO il tipo della colonna.
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
/// Usata dall'analisi delle trasformazioni che cambiano il tipo geometrico:
/// il contratto dichiara i tipi dell'output e il blocco canonico li
/// ri-emette da lì. La dichiarazione ereditata non deve sopravvivere
/// accanto (un consumatore a valle leggerebbe il tipo di prima della
/// trasformazione) né provocare un conflitto di chiave.
pub fn strip_rewritten_types_declarations<S: std::hash::BuildHasher>(
    metadata: &mut HashMap<String, String, S>,
) {
    for key in TYPES_KEYS_REWRITTEN_BY_TRANSFORM {
        metadata.remove(key);
    }
}

/// Rimuove le chiavi canoniche del CRS dai metadati di un campo geometria,
/// senza toccare il metadato legacy `geo` (gia' riscritto dall'operazione).
///
/// Per `geo.reproject`: la riproiezione cambia il CRS, quindi le chiavi
/// della sorgente si sostituiscono e il blocco canonico ri-emette il target
/// senza conflitto di chiave.
pub fn strip_rewritten_crs_keys<S: std::hash::BuildHasher>(
    metadata: &mut HashMap<String, String, S>,
) {
    for key in CRS_KEYS_REPLACED_BY_DECISION {
        metadata.remove(key);
    }
}

/// Rimuove le dichiarazioni CRS dai metadati di un campo geometria.
///
/// Toglie le chiavi di [`CRS_KEYS_REPLACED_BY_DECISION`] e il membro `crs`
/// del metadato `geo` (rimosso se resta vuoto), quando una decisione del
/// piano ([`ContractCrs::ResolvedByDecision`]) sostituisce le dichiarazioni
/// della sorgente. Un `geo` non oggetto o non JSON resta invariato: il
/// `geo` malformato è già un errore della lettura del contratto.
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
/// presente ma fuori dall'enumerazione chiusa → errore esplicito (mai
/// ignorare o correggere; il messaggio non riporta il valore, «errori senza
/// dati»: i messaggi dei tipi di `plenora-core` elencano solo i valori
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

/// Intero decimale senza segno: solo cifre ASCII, niente segno (il
/// `FromStr` di `u32` accetterebbe `+`), niente spazi, entro `u32`.
fn parse_unsigned_decimal(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Parsing di una chiave canonica a intero decimale senza segno.
fn parse_canonical_u32(raw: Option<&String>, key: &str) -> Result<Option<u32>, PlenoraError> {
    raw.map(|value| {
        parse_unsigned_decimal(value).ok_or_else(|| {
            PlenoraError::InvalidPlan(format!(
                "chiave `{key}`: atteso un intero decimale senza segno"
            ))
        })
    })
    .transpose()
}

/// Encoding canonico di una colonna geometrica.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la chiave è presente ma fuori dall'enum
/// chiuso (`wkb` | `ewkb`).
pub fn canonical_geometry_encoding(
    field: &Field,
) -> Result<Option<GeometryEncoding>, PlenoraError> {
    parse_canonical_enum(
        field.metadata().get(PLENORA_GEOMETRY_ENCODING_KEY),
        PLENORA_GEOMETRY_ENCODING_KEY,
    )
}

/// Dimensionalità canonica di una colonna geometrica (`unknown` è un valore
/// canonico, mai mappato a `xy`).
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

/// Coppia (`types_declaration`, `types`) canonica.
///
/// Le coerenze sono di [`GeometryTypesProperty::from_canonical_list`].
/// Entrambe le chiavi assenti danno `Ok(None)` («proprieta' non dichiarata»,
/// mai `unresolved`); `types` senza `types_declaration` e' un errore.
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` se la dichiarazione non e' canonica, se `types`
/// compare senza `types_declaration` o se la coppia non è coerente
/// ([`crate::contract::GeometryTypesPropertyError`]).
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
             `{PLENORA_GEOMETRY_TYPES_DECLARATION_KEY}`"
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

/// SRID canonico, se dichiarato (intero decimale senza segno).
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

/// Identificatore di autorità del CRS, validato come in
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

/// Stato di risoluzione del CRS.
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

/// Definizione CRS testuale e suo formato.
///
/// Le due chiavi devono essere presenti insieme, la definizione rispetta i
/// limiti testuali di [`MAX_CRS_DEFINITION_BYTES`] e la sua forma deve
/// corrispondere al formato dichiarato (una definizione incoerente non
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
             `{PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY}` devono essere presenti insieme"
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
                     corrisponde al formato `{format}` dichiarato"
                )));
            }
            Ok(Some((definition.clone(), format)))
        }
    }
}

/// Ordine degli assi canonico (`unknown` è un valore ammesso).
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

/// Semantica spaziale canonica.
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

/// Precisione delle coordinate canonica.
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

/// Identità logica stabile della colonna (intero decimale senza segno).
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

/// Versione del contratto dei metadati dichiarata dallo schema.
///
/// Presente: intero decimale senza segno; una versione maggiore di
/// [`PLENORA_CONTRACT_VERSION`] si rifiuta (mai un'interpretazione
/// parziale). Una versione minore si accetta: falliscono solo le
/// successive. Assente: errore se lo schema porta chiavi `plenora.`,
/// altrimenti `Ok(None)` (input senza chiavi canoniche).
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
                 `{PLENORA_CONTRACT_VERSION_KEY}` nei metadati dello schema"
            )));
        }
        return Ok(None);
    };
    let version = parse_unsigned_decimal(raw).ok_or_else(|| {
        PlenoraError::InvalidPlan(format!(
            "chiave `{PLENORA_CONTRACT_VERSION_KEY}`: atteso un intero decimale senza segno"
        ))
    })?;
    if version > PLENORA_CONTRACT_VERSION {
        return Err(PlenoraError::Unsupported(format!(
            "`{PLENORA_CONTRACT_VERSION_KEY}` successiva a {PLENORA_CONTRACT_VERSION}: \
             fallimento esplicito, mai un'interpretazione parziale"
        )));
    }
    Ok(Some(version))
}

/// Rileva la presenza di chiavi nel namespace canonico nei metadati dello
/// schema o di un qualunque campo (che rende obbligatoria la versione).
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
/// canoniche, la verifica di coerenza con il metadato `geo` e il
/// completamento per precedenza di [`read_geometry_contract_keys`].
///
/// Ogni nozione è `Option`: assente significa «non dichiarata», mai un
/// default. La provenienza (canonica, `geo` o standard esterno) non è
/// conservata: è decisa interamente durante la lettura.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalGeometryKeys {
    /// Framing binario delle celle.
    pub encoding: Option<GeometryEncoding>,
    /// Dimensionalità dichiarata (`unknown` = non risolta).
    pub dimensions: Option<GeometryDimensions>,
    /// Coppia (`types_declaration`, `types`).
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
/// assente; JSON non valido o valore non canonico → errore esplicito
/// («illeggibile» non è «assente», e un `geo` malformato non va scavalcato
/// dalle chiavi canoniche nel completamento per precedenza).
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

/// Divergenza fra chiavi canoniche e metadato legacy su una nozione (il
/// componente fallisce, non sceglie). Il messaggio nomina la nozione, mai i
/// valori («errori senza dati»).
fn divergent_geometry_keys(notion: &str) -> PlenoraError {
    PlenoraError::InvalidPlan(format!(
        "nozione `{notion}` divergente fra chiavi canoniche e metadato legacy `geo` \
         (il componente fallisce, non sceglie)"
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
/// 1. ogni chiave canonica e' letta dal suo reader tipizzato: assente ->
///    `None`, non canonica -> errore;
/// 2. coerenze fra chiavi canoniche: `axis_order` obbligatorio con `crs_id`
///    o `crs_definition` (`unknown` ammesso); `crs_resolution = missing`
///    esclude `crs_id`/`crs_definition`/`srid`/`axis_order`;
/// 3. una nozione presente sia canonica sia legacy deve coincidere
///    (il CRS a parita' di forma; forme non confrontabili contano come
///    divergenza): il componente fallisce, non sceglie;
/// 4. completamento, senza ispezionare i dati: canonica > legacy >
///    standard esterno (`geoarrow.wkb` completa `encoding` con `wkb`). Non si
///    controlla encoding contro estensione: EWKB e' un dialetto WKB e i
///    costruttori emettono legittimamente `ewkb` sotto quel nome.
///
/// Il controllo della versione del contratto spetta al chiamante
/// ([`read_contract_version`]).
///
/// # Errors
///
/// `PlenoraError::InvalidPlan` per valori canonici non validi, coerenze
/// intra-canoniche violate o divergenza canonica-vs-legacy;
/// `PlenoraError::Unsupported` per un encoding legacy non rappresentabile
/// (come [`geometry_encoding_from_metadata_strict`]).
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

    // Coerenze fra chiavi canoniche, verificate PRIMA del
    // completamento: riguardano la sola rappresentazione canonica, cosi' un
    // input legacy senza `axis_order` resta leggibile.
    if (keys.crs_id.is_some() || keys.crs_definition.is_some()) && keys.axis_order.is_none() {
        return Err(PlenoraError::InvalidPlan(format!(
            "chiave `{PLENORA_GEOMETRY_AXIS_ORDER_KEY}` obbligatoria quando \
             `{PLENORA_GEOMETRY_CRS_ID_KEY}` o `{PLENORA_GEOMETRY_CRS_DEFINITION_KEY}` \
             e' presente (valore `unknown` ammesso)"
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
             dichiarati"
        )));
    }

    let legacy = legacy_geo_keys(field)?;

    // Coerenza con il metadato `geo` e completamento, nozione per nozione.
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
        // Standard esterno (ultimo rango): il nome di estensione
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
            // CRS: divergenza.
            return Err(divergent_geometry_keys("crs"));
        }
        if keys.crs_id.is_none() && keys.crs_definition.is_none() {
            // Completamento dal rango legacy.
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
