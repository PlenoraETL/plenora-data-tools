//! Adapter Arrow per il canone GeoArrow-WKB (rappresentazione).
//!
//! Port Fase 1 da `arrow_transport.rs` di plenora-geo-tools-arrow, limitato
//! alle parti di rappresentazione: metadati di estensione GeoArrow
//! (`ARROW:extension:name` = `geoarrow.wkb`), metadato `geo` JSON con le
//! chiavi `crs`, (dalla milestone B1.1, ICD §3.3) `dimensions` e (dalla
//! milestone B1.4) `encoding` — scritta solo quando il contratto la
//! dichiara, mai come default — decode/encode delle celle WKB con limiti
//! per cella e helper
//! sui `RecordBatch`. L'envelope `PLNGEO3`, i checksum, la CLI e gli schemi
//! `TransformArrowSchema`/`PairArrowSchema` non fanno parte di questo modulo
//! (andranno in `plenora-engine`).
//!
//! Unificazione B1.1: questo modulo e' la casa unica dei metadati GeoArrow;
//! il trasporto Arrow v3 di `plenora-engine::geo_transport` delega qui
//! (stesso JSON in uscita byte-per-byte).
//!
//! Le geometrie viaggiano in una colonna `Binary`; ogni cella non-null e'
//! validata dal validatore WKB del kernel e i null sono preservati.
//!
//! Errori: le condizioni `ArrowTransportError` del sorgente sono mappate su
//! [`PlenoraError`] preservando i messaggi (colonne/schema → `Schema`, CRS →
//! `Crs`, limiti cella → `Contract`, serializzazione JSON metadati → `Json`).

use std::collections::HashMap;

use geo::Geometry;
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{Array, BinaryArray};
use plenora_core::arrow::{DataType, Field, RecordBatch, Schema};
use plenora_core::contract::{GeometryDimensions, GeometryEncoding};
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
    PlenoraError::Contract(format!(
        "cella WKB da {bytes} byte oltre il limite {MAX_CELL_BYTES}"
    ))
}

/// Indice della colonna geometria: deve esistere, essere `Binary` e portare
/// i metadati di estensione `geoarrow.wkb`.
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

/// Metadato GeoArrow `geo` con la chiave `crs`: PROJJSON se la definizione e'
/// gia' un oggetto JSON, altrimenti la forma authority:code come stringa.
///
/// Casa unica del formato (unificazione B1.1): anche il trasporto Arrow v3 di
/// `plenora-engine` delega qui, quindi il JSON in uscita e' identico
/// byte-per-byte nei due percorsi.
pub fn geo_metadata_json(crs: &str) -> Result<String, PlenoraError> {
    let metadata = geo_metadata_map(crs)?;
    serde_json::to_string(&serde_json::Value::Object(metadata)).map_err(PlenoraError::Json)
}

/// Come [`geo_metadata_json`], con in piu' la chiave `dimensions` in forma
/// ICD ([`GeometryDimensions::as_str`]). La propagazione reale della
/// dimensionalita' e' milestone B1.3: qui la scriviamo solo per dichiararla.
pub fn geo_metadata_json_with_dimensions(
    crs: &str,
    dimensions: GeometryDimensions,
) -> Result<String, PlenoraError> {
    geo_metadata_json_with_encoding(crs, dimensions, None)
}

/// Come [`geo_metadata_json_with_dimensions`], con in piu' la chiave
/// `encoding` in forma ICD ([`GeometryEncoding::as_str`]) quando il contratto
/// la dichiara (`Some`). Con `None` la chiave e' omessa e il JSON e'
/// identico byte-per-byte a [`geo_metadata_json_with_dimensions`]
/// (fingerprint e retrocompatibilita' invariati — B1.4).
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
    serde_json::to_string(&serde_json::Value::Object(metadata)).map_err(PlenoraError::Json)
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
pub fn geometry_output_field(name: &str, crs: &str) -> Result<Field, PlenoraError> {
    geometry_output_field_with_dimensions(name, crs, GeometryDimensions::Xy)
}

/// Come [`geometry_output_field`], con la dimensionalita' dichiarata
/// esplicitamente (pronto per la propagazione di B1.3).
pub fn geometry_output_field_with_dimensions(
    name: &str,
    crs: &str,
    dimensions: GeometryDimensions,
) -> Result<Field, PlenoraError> {
    geometry_output_field_with_encoding(name, crs, dimensions, None)
}

/// Come [`geometry_output_field_with_dimensions`], con in piu' la chiave
/// `geo.encoding` quando il contratto la dichiara (`Some`) — B1.4: un
/// contratto con encoding dichiarato che attraversa un kernel che riscrive
/// il campo (es. `reproject`) conserva la chiave nel metadato riscritto,
/// coerente col contratto. Con `None` la chiave e' omessa e il metadato e'
/// identico byte-per-byte alla forma senza encoding (fingerprint e
/// retrocompatibilita' invariati).
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
/// (B1.3): la chiave `encoding` presente ma fuori dall'enum chiuso (R3.5:
/// header GeoPackage, TWKB, valori non testuali) e' un framing non
/// rappresentabile e va rifiutato con errore esplicito — mai mappata a un
/// encoding noto o ignorata. Chiave assente o metadato `geo` non valido →
/// `Ok(None)` (la dimensionalita'/il framing non dichiarati restano non
/// risolti, R3.4; il messaggio non riporta il valore, «errori senza dati»).
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

/// Colonna geometria di un batch, gia' indicizzata da
/// [`geometry_column_index`]: deve essere un `BinaryArray`.
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
pub fn decode_geometry_cell(payload: &[u8]) -> Result<Geometry<f64>, PlenoraError> {
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(cell_too_large(payload.len() as u64));
    }
    geometry_from_wkb(payload)
}

/// Codifica una geometria gia' validata dal kernel in WKB 2D entro il limite
/// per cella.
pub fn encode_geometry(geometry: &Geometry<f64>) -> Result<Vec<u8>, PlenoraError> {
    let payload = geometry
        .to_wkb(CoordDimensions::xy())
        .map_err(|error| PlenoraError::Contract(format!("geometria prodotta non valida: {error}")))?;
    if payload.len() as u64 > MAX_CELL_BYTES {
        return Err(cell_too_large(payload.len() as u64));
    }
    Ok(payload)
}

/// Applica `f` a ogni cella non-null preservando i null; il limite per cella
/// e' applicato prima di toccare i dati. Le righe sono indipendenti:
/// l'iterazione e' parallela (rayon) con collect indicizzato, quindi
/// l'ordine dell'output resta deterministico.
pub fn map_nullable<T: Send>(
    cells: &BinaryArray,
    f: impl Fn(&[u8]) -> Result<Option<T>, PlenoraError> + Sync,
) -> Result<Vec<Option<T>>, PlenoraError> {
    let cell_values: Vec<Option<&[u8]>> = cells.iter().collect();
    cell_values
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
        .collect()
}

/// STIMA dei byte nativi delle geometrie decodificate di una colonna
/// geometria (ADR-0002, Fase 2B-M2b): decodifica ogni cella non-null e
/// somma le stime per cella. Il valore e' una STIMA dichiarata (formula in
/// [`crate::memory_estimate`]), da riportare nelle metriche come "memoria
/// nativa stimata", mai come conteggio preciso. I null contribuiscono zero.
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
/// cella decodificata in `accumulator` (punto naturale di raccolta della
/// metrica "stimata" per il governor; l'integrazione con `plenora-engine`
/// e' volutamente rimandata). Restituisce il totale corrente
/// dell'accumulatore, non il solo contributo di questa colonna.
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
    use std::sync::Arc;

    const CRS: &str = "EPSG:3857";

    fn fixture_batch(cells: &[Option<&Vec<u8>>]) -> (Schema, RecordBatch) {
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("geometry field"),
        ]);
        let ids = plenora_core::arrow::array::Int64Array::from_iter_values(0..cells.len() as i64);
        let geometry = BinaryArray::from_iter(cells.iter().map(|cell| cell.map(Vec::as_slice)));
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

        let oversized = vec![0_u8; (MAX_CELL_BYTES + 1) as usize];
        let cells = BinaryArray::from_iter([Some(oversized.as_slice())]);
        assert!(matches!(
            map_nullable(&cells, |payload| decode_geometry_cell(payload).map(Some)),
            Err(PlenoraError::Contract(_))
        ));
        assert!(matches!(
            decode_geometry_cell(&oversized),
            Err(PlenoraError::Contract(_))
        ));
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
        let cells = BinaryArray::from_iter(
            cells_payload.iter().map(|cell| cell.as_deref()),
        );

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
}
