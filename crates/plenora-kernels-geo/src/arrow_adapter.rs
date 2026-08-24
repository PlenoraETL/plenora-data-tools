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
//! `docs/errori-e-limiti.md` errori-e-limiti.md#limiti-dichiarati): protocollo delle chiavi canoniche
//! `plenora.geometry.*` e `plenora.contract.version` (R2.1/R2.2: namespace
//! dedicato, una chiave per nozione, MAI un blob unico). R4.6.3 (rc9/rc10):
//! l'emissione porta anche gli stati `crs_resolution = missing` (nessuna
//! chiave CRS, coerenza R2.2) e `declared_unresolved` (dichiarazioni
//! originali ri-emesse invariate, R4.6.4) — piano-v5.md#contratti-di-input decisione 7. `plenora.field_id`
//! e' solo letta (R2.2 opzionale; non si emette il `FieldId` di grafo, che
//! non ha significato fuori dal processo — piano-v5.md#contratti-di-input decisione 3). Questo
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
use plenora_core::crs::{
    authority_code_srid, definition_form, DefinitionForm, ResolvedCrs, MAX_CRS_DEFINITION_BYTES,
};
use plenora_core::PlenoraError;
use rayon::prelude::*;

use crate::geometry_from_wkb;
use crate::memory_estimate::{estimate_geometry_native_bytes, DecodedNativeBytesEstimate};

// Il codec dei metadati contrattuali vive ora in `plenora-core`: e' sotto sia
// alla CLI sia all'engine, come il protocollo del worker isolato richiede.
// Qui resta cio' che tocca le celle WKB, e il re-export tiene invariati i
// percorsi dei chiamanti.
pub use plenora_core::contract::arrow_metadata::*;

// Errori delle CELLE WKB: restano qui, con il codice che le legge.

fn cell_too_large(bytes: u64) -> PlenoraError {
    PlenoraError::ResourceLimit(format!(
        "cella WKB da {bytes} byte oltre il limite {MAX_CELL_BYTES}"
    ))
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
            geometry_column_not_binary(geometry_column, batch.column(geometry_index).data_type())
        })
}

/// Decodifica una cella WKB non-null: il limite per cella e' applicato prima
/// di toccare i dati, poi vale il contratto WKB strutturale del kernel.
///
/// # Errors
///
/// `PlenoraError::ResourceLimit` se il payload supera [`MAX_CELL_BYTES`]; in piu'
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
/// fallisce; `PlenoraError::ResourceLimit` se il payload prodotto supera
/// [`MAX_CELL_BYTES`].
pub fn encode_geometry(geometry: &Geometry<f64>) -> Result<Vec<u8>, PlenoraError> {
    let payload = geometry.to_wkb(CoordDimensions::xy()).map_err(|error| {
        PlenoraError::InvalidPlan(format!("geometria prodotta non valida: {error}"))
    })?;
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
/// `PlenoraError::ResourceLimit` se una cella non-null supera [`MAX_CELL_BYTES`];
/// in piu' l'errore restituito da `f` sulla prima cella IN ORDINE DI RIGA
/// che fallisce (deterministico, architettura.md#determinismo).
pub fn map_nullable<T: Send>(
    cells: &BinaryArray,
    f: impl Fn(&[u8]) -> Result<Option<T>, PlenoraError> + Sync,
) -> Result<Vec<Option<T>>, PlenoraError> {
    // architettura.md#determinismo: il collect parallelo di `Result` sceglie l'errore in modo
    // NON deterministico (il primo che acquisisce il mutex interno di
    // rayon). Qui i `Result` sono raccolti per riga (l'ordine del collect
    // parallelo indicizzato e' preservato) e il primo errore IN ORDINE DI
    // RIGA e' selezionato dal collect sequenziale: stesso input, stesso
    // errore, sempre. L'identita' dell'errore e' output.
    //
    // La parallelizzazione e' sugli INDICI: la versione precedente
    // materializzava prima un `Vec<Option<&[u8]>>` di tutte le celle solo per
    // avere un iteratore indicizzato: un'allocazione da una riga per elemento
    // su ogni colonna geometrica, in un percorso che gia' scorre l'array.
    let results: Vec<Result<Option<T>, PlenoraError>> = (0..cells.len())
        .into_par_iter()
        .map(|row| {
            if cells.is_null(row) {
                return Ok(None);
            }
            let payload = cells.value(row);
            if payload.len() as u64 > MAX_CELL_BYTES {
                return Err(cell_too_large(payload.len() as u64));
            }
            f(payload)
        })
        .collect();
    results.into_iter().collect()
}

/// STIMA dei byte nativi delle geometrie decodificate di una colonna
/// geometria (architettura.md#memoria, Fase 2B-M2b).
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
        decode_geometry_cell(payload)
            .map(|geometry| Some(estimate_geometry_native_bytes(&geometry)))
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

        assert!(matches!(geo_metadata_json("  "), Err(PlenoraError::Crs(_))));
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
        assert_eq!(
            geo.get("crs").and_then(serde_json::Value::as_str),
            Some(CRS)
        );
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
            (
                r#"{"crs":"EPSG:3857","encoding":"wkb"}"#,
                Some(GeometryEncoding::Wkb),
            ),
            (
                r#"{"crs":"EPSG:3857","encoding":"ewkb"}"#,
                Some(GeometryEncoding::Ewkb),
            ),
        ] {
            let mut metadata = HashMap::new();
            metadata.insert(GEO_METADATA_KEY.to_owned(), raw.to_owned());
            let field = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
            assert_eq!(
                geometry_encoding_from_metadata_strict(&field).unwrap(),
                expected
            );
        }
        let bare = Field::new("geom", DataType::Binary, true);
        assert_eq!(geometry_encoding_from_metadata_strict(&bare).unwrap(), None);
        // Metadato `geo` illeggibile: ERRORE, non «assente» (R5.1). Il
        // lettore strict e' quello di contratto: se non riesce a leggere il
        // rango legacy non puo' dichiararlo vuoto e lasciare che le chiavi
        // canoniche completino al suo posto.
        let mut metadata = HashMap::new();
        metadata.insert(GEO_METADATA_KEY.to_owned(), "non json".to_owned());
        let broken = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
        assert!(geometry_encoding_from_metadata_strict(&broken).is_err());
        // Il lettore opportunistico, che dichiara di non decidere, resta
        // tollerante: la nozione risulta non dichiarata.
        assert_eq!(geometry_encoding_from_metadata(&broken), None);
        let mut metadata = HashMap::new();
        metadata.insert(GEO_METADATA_KEY.to_owned(), geo_metadata_json(CRS).unwrap());
        let crs_only = Field::new("geom", DataType::Binary, true).with_metadata(metadata);
        assert_eq!(
            geometry_encoding_from_metadata_strict(&crs_only).unwrap(),
            None
        );
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
    fn geometry_column_index_accepts_the_canonical_only_form() {
        // piano-v5.md#contratti-di-input decisione 8 (minore 1): la forma a sole chiavi canoniche
        // (`plenora.geometry.encoding` + `plenora.geometry.dimensions`
        // bastano) identifica la colonna — l'estensione `geoarrow.wkb` e'
        // ammessa, non richiesta.
        let canonical_only = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(
                HashMap::from([
                    (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
                    (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
                ]),
            ),
        ]);
        assert_eq!(
            geometry_column_index(&canonical_only, DEFAULT_GEOMETRY_COLUMN).unwrap(),
            1
        );

        // Un nome di estensione DIVERSO dichiara un altro framing: rifiutato
        // anche in presenza di chiavi canoniche.
        let foreign_extension = Schema::new(vec![Field::new(
            DEFAULT_GEOMETRY_COLUMN,
            DataType::Binary,
            true,
        )
        .with_metadata(HashMap::from([
            (
                GEOARROW_EXTENSION_KEY.to_owned(),
                "geoarrow.point".to_owned(),
            ),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY.to_owned(), "xy".to_owned()),
        ]))]);
        assert!(matches!(
            geometry_column_index(&foreign_extension, DEFAULT_GEOMETRY_COLUMN),
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

        let oversized =
            vec![0_u8; usize::try_from(MAX_CELL_BYTES + 1).expect("64 MiB + 1 entra in usize")];
        let cells = BinaryArray::from_iter([Some(oversized.as_slice())]);
        assert!(matches!(
            map_nullable(&cells, |payload| decode_geometry_cell(payload).map(Some)),
            // Decimo giro: una cella oltre il tetto e' un limite di RISORSA
            // (il volume del dato), non un piano sbagliato.
            Err(PlenoraError::ResourceLimit(_))
        ));
        assert!(matches!(
            decode_geometry_cell(&oversized),
            Err(PlenoraError::ResourceLimit(_))
        ));
    }

    #[test]
    fn map_nullable_reports_the_first_failing_row_deterministically() {
        // architettura.md#determinismo: con piu' celle fallite, l'errore riportato DEVE essere
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

    /// STIMA per colonna geometria (architettura.md#memoria): somma delle stime delle celle
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

        let expected =
            estimate_geometry_native_bytes(&square) + estimate_geometry_native_bytes(&point);
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
        assert_eq!(
            get(PLENORA_GEOMETRY_AXIS_ORDER_KEY),
            Some("easting_northing")
        );
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), Some("3857"));
        assert_eq!(
            get(PLENORA_GEOMETRY_SPATIAL_SEMANTICS_KEY),
            Some("geometry")
        );
        assert_eq!(get(PLENORA_GEOMETRY_PRECISION_KEY), Some("float64"));
        // `field_id` non e' emesso (R2.2 opzionale; il FieldId di grafo non
        // ha significato fuori dal processo, piano-v5.md#contratti-di-input decisione 3).
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
        // axis_order = `unknown` perche' lo stub non dichiara gli assi.
        assert_eq!(get(PLENORA_GEOMETRY_DIMENSIONS_KEY), Some("xyz"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY), Some("resolved"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), Some("EPSG:3857"));
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("unknown"));
        // `field_id` non e' emesso (R2.2 opzionale; il FieldId di grafo non
        // ha significato fuori dal processo, piano-v5.md#contratti-di-input decisione 3).
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

    // --- Ordine fisico normalizzato + SRID d'autorita' (piano-v5.md#contratti-di-input,
    // emendamento 2026-08-01) ----------------------------------------------

    /// Contratto `Resolved` con PROJJSON realistico di EPSG:4326 (assi e
    /// `id` d'autorita' presenti — la forma prodotta dalla risoluzione PROJ).
    fn epsg_4326_realistic_contract() -> GeometryColumnContract {
        GeometryColumnContract {
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                "EPSG:4326".to_owned(),
                serde_json::json!({
                    "type": "GeographicCRS",
                    "name": "WGS 84",
                    "coordinate_system": {
                        "subtype": "ellipsoidal",
                        "axis": [
                            {"name": "Geodetic latitude", "abbreviation": "Lat",
                             "direction": "north", "unit": "degree"},
                            {"name": "Geodetic longitude", "abbreviation": "Lon",
                             "direction": "east", "unit": "degree"},
                        ],
                    },
                    "id": {"authority": "EPSG", "code": 4326},
                }),
                CrsKind::Geographic,
                None,
            )),
            types: GeometryColumnContract::undeclared_types(),
            encoding: None,
            ..full_contract()
        }
    }

    #[test]
    fn canonical_metadata_uses_normalized_axis_order_and_authority_srid() {
        // Completamento DELL'ASSENTE (R2.7): senza dettagli espliciti,
        // axis_order descrive i byte x/y normalizzati (EPSG:4326 -> lon_lat),
        // mentre lo srid resta dedotto dall'autorita' (id EPSG:4326 -> 4326).
        // Lo stub `{"type":"ProjectedCRS"}`
        // della fixture storica resta coperto da
        // `canonical_metadata_minimal_omits_everything_not_declared`
        // (comportamento precedente preservato: `unknown` + niente srid).
        let metadata = canonical_geometry_metadata(
            &epsg_4326_realistic_contract(),
            &GeometryMetadataDetails::default(),
        );
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("lon_lat"));
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), Some("4326"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), Some("EPSG:4326"));
    }

    #[test]
    fn canonical_metadata_explicit_details_win_over_authority_deduction() {
        // R2.7 completa solo l'assente: un dettaglio esplicito vince SEMPRE
        // sulla deduzione d'autorita', anche se diverge.
        let details = GeometryMetadataDetails {
            axis_order: Some(AxisOrder::LonLat),
            srid: Some(84),
            ..GeometryMetadataDetails::default()
        };
        let metadata = canonical_geometry_metadata(&epsg_4326_realistic_contract(), &details);
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("lon_lat"));
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), Some("84"));
    }

    #[test]
    fn legacy_resolved_definition_deduces_srid_but_not_axis_order() {
        // Trasporto legacy (nessun `ResolvedCrs`): lo `srid` e' dedotto
        // dalla forma `authority:code` della definizione; `axis_order`
        // resta `unknown` — limite dichiarato (il trasporto non risolve la
        // definizione, dedurre gli assi sarebbe inventarli).
        let metadata = canonical_geometry_metadata_for_resolved_definition(
            "EPSG:4326",
            GeometryDimensions::Xy,
            None,
            &GeometryMetadataDetails::default(),
        );
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), Some("4326"));
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("unknown"));
        // Codice non numerico: nessuno srid; dettaglio esplicito vincente.
        let metadata = canonical_geometry_metadata_for_resolved_definition(
            "OGC:CRS84",
            GeometryDimensions::Xy,
            None,
            &GeometryMetadataDetails::default(),
        );
        assert_eq!(metadata.get(PLENORA_GEOMETRY_SRID_KEY), None);
        let details = GeometryMetadataDetails {
            srid: Some(7),
            ..GeometryMetadataDetails::default()
        };
        let metadata = canonical_geometry_metadata_for_resolved_definition(
            "EPSG:4326",
            GeometryDimensions::Xy,
            None,
            &details,
        );
        assert_eq!(
            metadata.get(PLENORA_GEOMETRY_SRID_KEY).map(String::as_str),
            Some("7"),
            "R2.7: il dettaglio esplicito vince sulla deduzione"
        );
    }

    // --- Emissione da definizione WKT (piano-v5.md#contratti-di-input, emendamento 2026-07-31 —
    // classe B) ----------------------------------------------------------

    /// WKT1 realistico di Monte Mario / Italy zone 1 con `AUTHORITY` e
    /// `TOWGS84` (EPSG:3003): la forma dello shapefile catastale owner.
    const MONTE_MARIO_WKT: &str = concat!(
        r#"PROJCS["Monte Mario / Italy zone 1",GEOGCS["Monte Mario","#,
        r#"DATUM["Monte_Mario",SPHEROID["International 1924",6378388,297],"#,
        r#"TOWGS84[-104.1,-49.1,-9.9,0.971,-2.917,0.714,-11.68]],"#,
        r#"PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],"#,
        r#"PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],"#,
        r#"PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],"#,
        r#"PARAMETER["false_easting",1500000],PARAMETER["false_northing",0],"#,
        r#"UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],"#,
        r#"AUTHORITY["EPSG","3003"]]"#
    );

    /// Contratto `Resolved` la cui definizione e' WKT (il kernel ha
    /// risolto il testo WKT contro PROJ; il canonical porta assi e `id`).
    fn monte_mario_wkt_contract() -> GeometryColumnContract {
        GeometryColumnContract {
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                MONTE_MARIO_WKT.to_owned(),
                serde_json::json!({
                    "type": "ProjectedCRS",
                    "name": "Monte Mario / Italy zone 1",
                    "coordinate_system": {
                        "subtype": "Cartesian",
                        "axis": [
                            {"name": "Easting", "abbreviation": "E",
                             "direction": "east", "unit": "metre"},
                            {"name": "Northing", "abbreviation": "N",
                             "direction": "north", "unit": "metre"},
                        ],
                    },
                    "id": {"authority": "EPSG", "code": 3003},
                }),
                CrsKind::Projected,
                Some(1.0),
            )),
            types: GeometryColumnContract::undeclared_types(),
            encoding: None,
            ..full_contract()
        }
    }

    #[test]
    fn canonical_metadata_from_wkt_definition_emits_definition_and_wkt_format() {
        // Classe B: una definizione WKT e' emessa come `crs_definition`
        // (byte originali) + `crs_definition_format = wkt`, MAI in
        // `crs_id` (passthrough R2.6 contro la lineage WKT); deduzione
        // d'autorita' dal canonical realistico (assi + id) gratis.
        let metadata = canonical_geometry_metadata(
            &monte_mario_wkt_contract(),
            &GeometryMetadataDetails::default(),
        );
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY),
            Some(MONTE_MARIO_WKT)
        );
        assert_eq!(get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY), Some("wkt"));
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), None);
        assert_eq!(
            get(PLENORA_GEOMETRY_AXIS_ORDER_KEY),
            Some("easting_northing")
        );
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), Some("3003"));
    }

    #[test]
    fn wkt_definition_is_coherent_with_identical_legacy_geo_crs() {
        let mut metadata = canonical_geometry_metadata(
            &monte_mario_wkt_contract(),
            &GeometryMetadataDetails::default(),
        );
        metadata.insert(
            GEO_METADATA_KEY.to_owned(),
            serde_json::json!({"crs": MONTE_MARIO_WKT}).to_string(),
        );
        let field = Field::new("geometry", DataType::Binary, true).with_metadata(metadata);

        let keys = read_geometry_contract_keys(&field)
            .expect("lo stesso WKT canonico e legacy descrive un CRS coerente");
        assert_eq!(keys.crs_definition.as_deref(), Some(MONTE_MARIO_WKT));
        assert_eq!(keys.crs_definition_format, Some(CrsDefinitionFormat::Wkt));
        assert_eq!(keys.crs_id, None);
    }

    #[test]
    fn canonical_metadata_from_wkt2_definition_emits_wkt2_format() {
        let wkt2 = r#"PROJCRS["WGS 84 / UTM zone 32N",BASEGEOGCRS["WGS 84"]]"#;
        let contract = GeometryColumnContract {
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                wkt2.to_owned(),
                serde_json::json!({"type": "ProjectedCRS"}),
                CrsKind::Projected,
                Some(1.0),
            )),
            types: GeometryColumnContract::undeclared_types(),
            encoding: None,
            ..full_contract()
        };
        let metadata = canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
        let get = |key: &str| metadata.get(key).map(String::as_str);
        assert_eq!(get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY), Some(wkt2));
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY),
            Some("wkt2")
        );
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), None);
        // Canonical stub senza assi: il valore onesto resta `unknown`.
        assert_eq!(get(PLENORA_GEOMETRY_AXIS_ORDER_KEY), Some("unknown"));
        assert_eq!(get(PLENORA_GEOMETRY_SRID_KEY), None);
    }

    #[test]
    fn wkt_root_aliases_are_accepted_and_emitted_as_definitions() {
        let cases = [
            ("FITTED_CS", CrsDefinitionFormat::Wkt),
            ("GEODETICCRS", CrsDefinitionFormat::Wkt2),
            ("GEOGRAPHICCRS", CrsDefinitionFormat::Wkt2),
            ("PROJECTEDCRS", CrsDefinitionFormat::Wkt2),
            ("VERTICALCRS", CrsDefinitionFormat::Wkt2),
            ("ENGINEERINGCRS", CrsDefinitionFormat::Wkt2),
        ];
        for delimiter in ['[', '('] {
            let closing = if delimiter == '[' { ']' } else { ')' };
            for (root, format) in cases {
                let definition = format!("{root}{delimiter}\"test\"{closing}");
                let field = field_with_pairs(&[
                    (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, definition.as_str()),
                    (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, format.as_str()),
                    (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
                ]);
                assert_eq!(
                    canonical_geometry_crs_definition(&field).expect("definizione valida"),
                    Some((definition.clone(), format)),
                    "{definition}"
                );

                let contract = GeometryColumnContract {
                    crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                        definition.clone(),
                        serde_json::json!({"type": "GeographicCRS"}),
                        CrsKind::Geographic,
                        None,
                    )),
                    types: GeometryColumnContract::undeclared_types(),
                    encoding: None,
                    ..full_contract()
                };
                let metadata =
                    canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
                assert_eq!(
                    metadata.get(PLENORA_GEOMETRY_CRS_DEFINITION_KEY),
                    Some(&definition),
                    "{definition} non deve finire in crs_id"
                );
                assert_eq!(
                    metadata
                        .get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY)
                        .map(String::as_str),
                    Some(format.as_str())
                );
                assert!(!metadata.contains_key(PLENORA_GEOMETRY_CRS_ID_KEY));
            }
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
        assert_eq!(
            get(PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY),
            Some("projjson")
        );
        assert_eq!(get(PLENORA_GEOMETRY_CRS_ID_KEY), None);
    }

    #[test]
    fn contract_keys_roundtrip_emit_then_read() {
        let field = Field::new("geom", DataType::Binary, true).with_metadata(
            canonical_geometry_metadata(&full_contract(), &full_details()),
        );
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
            .with_metadata(canonical_geometry_metadata(
                &full_contract(),
                &full_details(),
            ))
            .metadata()
            .clone();
        received.insert(PLENORA_FIELD_ID_KEY.to_owned(), "7".to_owned());
        let field = Field::new("geom", DataType::Binary, true).with_metadata(received);
        let keys = read_geometry_contract_keys(&field).expect("read con field_id");
        assert_eq!(keys.field_id, Some(FieldId(7)));

        // Round-trip PROJJSON: la definizione sopravvive byte-per-byte.
        let projjson = r#"{"type":"ProjectedCRS","name":"demo"}"#;
        let field =
            Field::new("geom", DataType::Binary, true).with_metadata(canonical_geometry_metadata(
                &GeometryColumnContract {
                    crs: ContractCrs::Resolved(resolved_crs(projjson)),
                    ..full_contract()
                },
                &full_details(),
            ));
        let keys = read_geometry_contract_keys(&field).expect("read projjson");
        assert_eq!(keys.crs_definition.as_deref(), Some(projjson));
        assert_eq!(
            keys.crs_definition_format,
            Some(CrsDefinitionFormat::Projjson)
        );
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
            &[
                (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, "EPSG:4326"),
                (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt"),
                (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            ][..],
            &[
                (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, "PROJCS[demo]"),
                (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt2"),
                (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            ][..],
            &[
                (PLENORA_GEOMETRY_CRS_DEFINITION_KEY, "PROJCRS[demo]"),
                (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "wkt"),
                (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
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
            (
                GEO_METADATA_KEY,
                r#"{"crs":"EPSG:3857","dimensions":"xyz"}"#,
            ),
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
            (
                GEO_METADATA_KEY,
                r#"{"crs":{"type":"GeographicCRS","name":"WGS 84"}}"#,
            ),
        ]);
        assert!(read_geometry_contract_keys(&incomparable).is_err());
        // PROJJSON canonico vs oggetto legacy diverso -> divergenza.
        let divergent_projjson = field_with_pairs(&[
            (
                PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                r#"{"type":"ProjectedCRS","name":"a"}"#,
            ),
            (PLENORA_GEOMETRY_CRS_DEFINITION_FORMAT_KEY, "projjson"),
            (PLENORA_GEOMETRY_AXIS_ORDER_KEY, "unknown"),
            (
                GEO_METADATA_KEY,
                r#"{"crs":{"type":"ProjectedCRS","name":"b"}}"#,
            ),
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
        assert_eq!(
            keys.crs_definition_format,
            Some(CrsDefinitionFormat::Projjson)
        );
        let definition = keys.crs_definition.as_deref().expect("definition");
        let parsed: serde_json::Value = serde_json::from_str(definition).expect("json");
        assert_eq!(parsed["type"], "GeographicCRS");
        assert_eq!(keys.crs_id, None);

        // Entrambe presenti e coerenti: ok; l'ordine delle chiavi PROJJSON
        // diverso non conta (confronto per valore JSON, non per stringa).
        let coherent = field_with_pairs(&[
            (
                PLENORA_GEOMETRY_CRS_DEFINITION_KEY,
                r#"{"name":"a","type":"ProjectedCRS"}"#,
            ),
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

        // Metadato legacy ILLEGGIBILE: errore, non rango legacy assente
        // (R5.1/piano-v5.md#contratti-di-input). Prima il JSON malformato era indistinguibile
        // dall'assenza, e la risoluzione completava per precedenza dalle sole
        // chiavi canoniche scavalcando in silenzio un legacy che non era
        // riuscita a leggere.
        let broken = field_with_pairs(&[(GEO_METADATA_KEY, "non json")]);
        assert!(read_geometry_contract_keys(&broken).is_err());

        // Vale anche in presenza di chiavi canoniche valide: sono proprio i
        // casi in cui il legacy rotto veniva ignorato.
        let broken_with_canonical = field_with_pairs(&[
            (GEO_METADATA_KEY, "non json"),
            (PLENORA_GEOMETRY_ENCODING_KEY, "wkb"),
            (PLENORA_GEOMETRY_DIMENSIONS_KEY, "xy"),
        ]);
        assert!(read_geometry_contract_keys(&broken_with_canonical).is_err());

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
