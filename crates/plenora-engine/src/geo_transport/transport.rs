//! Trasporto Arrow v3: envelope checksummed con payload Arrow IPC e
//! geometrie GeoArrow-WKB.
//!
//! Layout dell'envelope `PLNGEO3`:
//!
//! ```text
//! offset 0   magic "PLNGEO3\0"          (8 byte)
//! offset 8   payload_len uint64 LE      (8 byte)
//! offset 16  payload Arrow IPC stream   (payload_len byte)
//! ...        trailer "GEOEND3\0"        (8 byte)
//! ...        SHA-256(magic || len || payload) (32 byte)
//! ...        EOF: byte residui rifiutati
//! ```
//!
//! Le geometrie viaggiano in una colonna `Binary` con metadati di estensione
//! `GeoArrow` (`ARROW:extension:name` = `geoarrow.wkb`) e metadato `geo` JSON
//! con la chiave `crs`. Ogni cella non-null viene validata con il validatore
//! WKB del kernel; i null sono preservati. Il modulo e' puro I/O su
//! `Read`/`Write`: la verifica semantica del CRS e la pubblicazione atomica
//! restano nel livello comandi.
//!
//! Operazioni 1:1: `centroid`, `convex_hull`, `envelope`, `buffer`,
//! `simplify`, `boundary`, `point_on_surface`, `make_valid` (richiede
//! `geos-backend`) e `reproject` (richiede `proj-backend`) producono una
//! colonna geometria GeoArrow-WKB; `area`, `length`, `perimeter` producono
//! Float64, `vertex_count` `UInt64`, `bounds` quattro colonne Float64
//! `<geometry_column>_minx/miny/maxx/maxy`, `to_wkt` Utf8.

pub const ENVELOPE_MAGIC: &[u8; 8] = b"PLNGEO3\0";
pub const ENVELOPE_TRAILER_MAGIC: &[u8; 8] = b"GEOEND3\0";
// Costanti dei metadati GeoArrow: casa unica in `arrow_adapter`
// (unificazione B1.1), qui ri-esportate per compatibilita' di percorso.
pub use plenora_kernels_geo::arrow_adapter::{
    DEFAULT_GEOMETRY_COLUMN, GEOARROW_EXTENSION_KEY, GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY,
};
pub const DEFAULT_X_COLUMN: &str = "x";
pub const DEFAULT_Y_COLUMN: &str = "y";
pub const PARENT_INDEX_COLUMN: &str = "__parent_index";
pub const LEFT_INDEX_COLUMN: &str = "__left_index";
pub const RIGHT_INDEX_COLUMN: &str = "__right_index";
pub const DISTANCE_COLUMN: &str = "distance";
pub const WITHIN_COLUMN: &str = "within";
pub const COUNT_COLUMN: &str = "count";
pub const CLASS_COLUMN: &str = "__class";
/// Lavoro massimo di noding GEOS per `polygonize` e `split` poligonale.
pub const MAX_NODING_WORK: u64 = 100_000_000;
/// Test di intersezione massimi per `split` lineare.
pub const MAX_SPLIT_WORK: u64 = 100_000_000;

/// Vertici totali massimi elaborati da `clean_topology` sull'intera tabella.
pub const MAX_CLEAN_VERTICES: u64 = 100_000_000;
/// Metadati massimi di un singolo messaggio Arrow IPC (schema compreso).
pub const MAX_IPC_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub const AREA_COLUMN: &str = "area";
pub const WKT_COLUMN: &str = "wkt";
pub const DEFAULT_MAX_POINTS: u64 = 100_000;
pub const MAX_COLUMNS: usize = 1024;
pub const MAX_BATCHES: usize = 65_536;
pub const MAX_CELL_BYTES: u64 = 64 * 1024 * 1024;
/// Coordinate massime per cella: una cella da 64 MiB contiene al piu' 16 byte
/// per coordinata XY.
///
/// Scelta B1.3 (come in `arrow_adapter::MAX_CELL_COORDINATES`): bound
/// conservativo non stride-aware — con Z/M il reale e' minore, quindi il
/// bound e' permissivo ma sicuro; `Unknown` (R3.4) non ha stride garantito.
pub const MAX_CELL_COORDINATES: u64 = MAX_CELL_BYTES / 16;

// Re-export del perimetro pubblico originale del modulo: la scomposizione
// in sottomoduli (error, schema, envelope, ipc, unary, pair) e' meccanica e
// nessun percorso `transport::...` esterno cambia.
pub use super::envelope::{EnvelopeReader, EnvelopeWriter};
pub use super::error::ArrowTransportError;
pub use super::ipc::{decode_ipc, encode_ipc, encode_ipc_file};
pub use super::pair::{
    decode_geometry_batches, pair_arrow, pair_arrow_with_format, preflight_decoded_bytes,
    GeometryDecodeError, PairArrowSchema, PairArrowSummary, PairOperation,
};
pub use super::schema::{
    ArrowOperation, ArrowOutputFormat, ArrowShape, BufferCap, SimplifyPolicyParam,
    TransformArrowSchema, TransformArrowSummary,
};
pub use super::unary::{
    one_to_one_batch_prepared, prepare_one_to_one, transform_arrow, transform_arrow_with_format,
    transform_batches, OneToOnePrepared,
};

#[cfg(test)]
use super::protocol::MAX_ROWS;
#[cfg(test)]
use geozero::{CoordDimensions, ToWkb};
#[cfg(test)]
use plenora_core::arrow::array::{
    Array, BinaryArray, Float64Array, RecordBatch, StringArray, UInt64Array,
};
#[cfg(test)]
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
#[cfg(test)]
use plenora_core::crs::MAX_CRS_DEFINITION_BYTES;
#[cfg(test)]
use plenora_kernels_geo::operations::to_wkt;
#[cfg(test)]
use plenora_kernels_geo::predicates::SpatialPredicate;
#[cfg(test)]
use plenora_kernels_geo::spatial_join::JoinPredicate;
#[cfg(test)]
use plenora_kernels_geo::topology::OverlayMode;
#[cfg(test)]
use plenora_kernels_geo::{geometry_from_wkb, transform_wkb, Operation};

#[cfg(test)]
// Confronti float esatti intenzionali: le fixture sono costruite per
// produrre valori esatti (coordinate note, round-trip bit-esatti); il
// confronto per bit e' il contratto verificato, non un'approssimazione.
#[allow(clippy::float_cmp)]
mod tests {
    use super::super::unary::{geo_metadata_json, geometry_output_field};
    use super::*;
    use geo::{line_string, polygon, Area, CoordsIter, Geometry, Point};
    use plenora_core::arrow::array::Int64Array;
    use plenora_core::diagnostics::RowDiagnosticsCompleteness;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::Arc;

    const CRS: &str = "EPSG:3857";

    fn square_wkb(size: f64) -> Vec<u8> {
        Geometry::Polygon(polygon![
            (x: 0.0, y: 0.0), (x: size, y: 0.0),
            (x: size, y: size), (x: 0.0, y: size),
            (x: 0.0, y: 0.0),
        ])
        .to_wkb(CoordDimensions::xy())
        .expect("fixture WKB")
    }

    fn line_wkb() -> Vec<u8> {
        Geometry::LineString(line_string![
            (x: 0.0, y: 0.0),
            (x: 3.0, y: 0.0),
            (x: 3.0, y: 4.0),
        ])
        .to_wkb(CoordDimensions::xy())
        .expect("fixture WKB")
    }

    fn geometry_field() -> Field {
        let mut metadata = HashMap::new();
        metadata.insert(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        );
        Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata)
    }

    fn fixture_batch(geometries: &[Option<&[u8]>]) -> (SchemaRef, RecordBatch) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("label", DataType::Utf8, true),
            Field::new("weight", DataType::Float64, true),
            geometry_field(),
        ]));
        let rows = geometries.len();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                // righe fixture: poche per costruzione, entro i64.
                Arc::new(Int64Array::from(
                    (0..i64::try_from(rows).expect("righe fixture entro i64"))
                        .collect::<Vec<i64>>(),
                )),
                Arc::new(StringArray::from(
                    (0..rows)
                        .map(|index| Some(format!("riga-{index}")))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    (0..rows)
                        .map(|index| {
                            // fixture di test: `rows` e' il numero di
                            // geometrie passate alla fixture (poche unita'),
                            // ampiamente entro 2^53: conversione esatta.
                            #[allow(clippy::cast_precision_loss)]
                            let half = index as f64 * 0.5;
                            Some(half)
                        })
                        .collect::<Vec<_>>(),
                )),
                Arc::new(geometries.iter().copied().collect::<BinaryArray>()),
            ],
        )
        .expect("fixture batch");
        (schema, batch)
    }

    fn envelope_bytes(schema: &SchemaRef, batches: &[RecordBatch]) -> Vec<u8> {
        let payload = encode_ipc(schema, batches).expect("encode");
        let mut writer = EnvelopeWriter::new(Vec::new(), payload.len() as u64).expect("writer");
        writer.write_payload(&payload).expect("payload");
        writer.finish().expect("finish").0
    }

    fn arrow_schema(row_count: u64, operation: ArrowOperation) -> TransformArrowSchema {
        TransformArrowSchema {
            schema_version: TransformArrowSchema::VERSION,
            operation,
            row_count,
            crs: Some(CRS.to_owned()),
            geometry_column: None,
            distance: None,
            cap: None,
            tolerance: None,
            simplify_policy: None,
            target_crs: None,
            max_output_rows: None,
            max_points: None,
            x_column: None,
            y_column: None,
            snap_tolerance: None,
            remove_overlaps: None,
            fill_gaps: None,
            coefficients: None,
            x_offset: None,
            y_offset: None,
            x_factor: None,
            y_factor: None,
            degrees: None,
            x_origin: None,
            y_origin: None,
            concavity: None,
            length_threshold: None,
            max_segment_length: None,
            grid_size: None,
            start_ratio: None,
            end_ratio: None,
            ratio: None,
            node_input: None,
            require_complete: None,
        }
    }

    fn run(schema: &TransformArrowSchema, input: &[u8]) -> Result<Vec<u8>, ArrowTransportError> {
        let mut output = Vec::new();
        transform_arrow(input, &mut output, schema)?;
        Ok(output)
    }

    fn decode_output(output: &[u8]) -> (SchemaRef, Vec<RecordBatch>) {
        let payload = EnvelopeReader::new(output)
            .expect("envelope")
            .read_payload()
            .expect("payload");
        decode_ipc(&payload).expect("ipc")
    }

    fn single_cell_output(output: &[u8], column: &str) -> (SchemaRef, RecordBatch, usize) {
        let (schema, batches) = decode_output(output);
        let index = schema.index_of(column).expect("colonna output");
        (schema, batches.into_iter().next().expect("batch"), index)
    }

    #[test]
    fn geometry_roundtrip_preserves_nulls_attributes_and_crs_metadata() {
        let square = square_wkb(4.0);
        let (schema, batch) = fixture_batch(&[Some(&square), None, Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(3, ArrowOperation::Centroid), &input).expect("transform");

        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches.len(), 1);
        let out_batch = &out_batches[0];
        assert_eq!(out_batch.num_rows(), 3);

        let geometry_index = out_schema
            .index_of(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column");
        let field = out_schema.field(geometry_index);
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

        let cells = out_batch
            .column(geometry_index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("Binary column");
        let expected = transform_wkb(Operation::Centroid, &square).expect("kernel");
        assert_eq!(cells.value(0), expected.as_slice());
        assert!(cells.is_null(1));
        assert_eq!(cells.value(2), expected.as_slice());
        let centroid = geometry_from_wkb(cells.value(0)).expect("decode centroid");
        assert_eq!(centroid, Geometry::Point(Point::new(2.0, 2.0)));

        let ids = out_batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64");
        assert_eq!(ids.values(), &[0, 1, 2]);
        let labels = out_batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8");
        assert_eq!(labels.value(2), "riga-2");
        let weights = out_batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("Float64");
        assert_eq!(weights.value(1), 0.5);
    }

    // --- BLOCK-06: doppia emissione delle chiavi canoniche §2 (parita' v4) --

    /// Metadati canonici (`plenora.*`) di un campo, come mappa a se' stante.
    fn canonical_block(field: &Field) -> HashMap<String, String> {
        field
            .metadata()
            .iter()
            .filter(|(key, _)| key.starts_with("plenora."))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Envelope di un lato pair con metadati di campo e di schema su misura.
    fn side_envelope_with_metadata(
        field_metadata: HashMap<String, String>,
        schema_metadata: HashMap<String, String>,
        geometries: &[Option<&[u8]>],
    ) -> Vec<u8> {
        let schema = Arc::new(Schema::new_with_metadata(
            vec![Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true)
                .with_metadata(field_metadata)],
            schema_metadata,
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(
                geometries.iter().copied().collect::<BinaryArray>(),
            )],
        )
        .expect("batch");
        envelope_bytes(&schema, &[batch])
    }

    fn geoarrow_field_metadata() -> HashMap<String, String> {
        HashMap::from([(
            GEOARROW_EXTENSION_KEY.to_owned(),
            GEOARROW_WKB_EXTENSION.to_owned(),
        )])
    }

    #[test]
    fn transform_arrow_emits_canonical_keys_in_dual_emission() {
        let square = square_wkb(4.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(1, ArrowOperation::Centroid), &input).expect("transform");

        let (out_schema, out_batches) = decode_output(&output);
        let field = out_schema
            .column_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column")
            .1;
        let canonical = canonical_block(field);
        assert_eq!(
            canonical
                .get("plenora.geometry.dimensions")
                .map(String::as_str),
            Some("xy")
        );
        assert_eq!(
            canonical
                .get("plenora.geometry.crs_resolution")
                .map(String::as_str),
            Some("resolved")
        );
        assert_eq!(
            canonical.get("plenora.geometry.crs_id").map(String::as_str),
            Some(CRS)
        );
        assert_eq!(
            canonical
                .get("plenora.geometry.axis_order")
                .map(String::as_str),
            Some("unknown")
        );
        assert_eq!(
            canonical
                .get("plenora.geometry.encoding")
                .map(String::as_str),
            Some("wkb")
        );
        assert_eq!(
            canonical.get("plenora.geometry.types").map(String::as_str),
            Some("point")
        );
        assert_eq!(
            canonical
                .get("plenora.geometry.types_declaration")
                .map(String::as_str),
            Some("exact")
        );
        // Doppia emissione: le chiavi GeoArrow standard restano invariate.
        assert_eq!(
            field
                .metadata()
                .get(GEOARROW_EXTENSION_KEY)
                .map(String::as_str),
            Some(GEOARROW_WKB_EXTENSION)
        );
        assert!(field.metadata().contains_key(GEO_METADATA_KEY));
        // Versione di protocollo sullo schema (R2.5) e batch rivestiti.
        assert_eq!(
            out_schema
                .metadata()
                .get("plenora.contract.version")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(out_batches[0].schema().metadata(), out_schema.metadata());
    }

    #[test]
    fn transform_arrow_canonical_block_is_byte_identical_to_v4_form() {
        use plenora_core::contract::{
            ContractCrs, ContractProperty, FieldId, GeometryColumnContract, GeometryEncoding,
            GeometryType, GeometryTypesProperty, PropertyConfidence, PropertyScope,
            TypesDeclaration,
        };
        use plenora_core::crs::{CrsKind, ResolvedCrs};
        use plenora_kernels_geo::arrow_adapter::{
            canonical_geometry_metadata, GeometryMetadataDetails,
        };

        let square = square_wkb(4.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(1, ArrowOperation::Centroid), &input).expect("transform");
        let (out_schema, _) = decode_output(&output);
        let field = out_schema
            .column_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column")
            .1;

        // Forma v4 a parita' di input: stesso contratto che la discovery
        // costruirebbe per questa colonna (CRS risolto, dimensions xy,
        // encoding non dichiarato, tipi non dichiarati). Il canonical porta
        // l'`id` d'autorita' (forma della risoluzione PROJ): la deduzione
        // `srid` (ADR-0009, emendamento 2026-07-31) produce 3857 su
        // ENTRAMBI i percorsi — legacy dalla forma `authority:code` della
        // definizione, v4 dall'`id` del canonical — e l'identita' regge.
        // Senza `coordinate_system` anche `axis_order` coincide (`unknown`):
        // con gli assi presenti il v4 dedurrebbe mentre il legacy resta
        // `unknown` — LIMITE DICHIARATO del trasporto legacy (coperto dai
        // test di `arrow_adapter`), per questo la forma di questo fixture
        // non porta gli assi.
        let contract = GeometryColumnContract {
            field_id: FieldId(0),
            name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                CRS.to_owned(),
                serde_json::json!({
                    "type": "ProjectedCRS",
                    "name": "WGS 84 / Pseudo-Mercator",
                    "id": {"authority": "EPSG", "code": 3857},
                }),
                CrsKind::Projected,
                Some(1.0),
            )),
            dimensions: plenora_core::contract::GeometryDimensions::Xy,
            encoding: Some(GeometryEncoding::Wkb),
            nullable: true,
            types: ContractProperty::new(
                PropertyConfidence::Declared(
                    GeometryTypesProperty::new(TypesDeclaration::Exact, vec![GeometryType::Point])
                        .expect("tipo esatto coerente"),
                ),
                PropertyScope::Schema,
            ),
        };
        let expected = canonical_geometry_metadata(&contract, &GeometryMetadataDetails::default());
        assert_eq!(canonical_block(field), expected);
    }

    #[test]
    fn pair_passthrough_derives_canonical_block_from_geo_metadata() {
        // Campo geometria propagato invariato (within) con `geo` legacy:
        // il blocco canonico e' derivato dalle stesse dichiarazioni.
        let mut field_metadata = geoarrow_field_metadata();
        field_metadata.insert(
            GEO_METADATA_KEY.to_owned(),
            r#"{"crs":"EPSG:3857","dimensions":"xy"}"#.to_owned(),
        );
        let left = side_envelope_with_metadata(
            field_metadata,
            HashMap::new(),
            &[Some(&point_wkb(0.5, 0.5))],
        );
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0))]);
        let schema = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Within, 1, 1)
        };
        let output = run_pair(&schema, &left, &right).expect("within");
        let (out_schema, _) = decode_output(&output);
        let field = out_schema
            .column_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column")
            .1;
        let canonical = canonical_block(field);
        assert_eq!(
            canonical
                .get("plenora.geometry.dimensions")
                .map(String::as_str),
            Some("xy")
        );
        assert_eq!(
            canonical
                .get("plenora.geometry.crs_resolution")
                .map(String::as_str),
            Some("resolved")
        );
        assert_eq!(
            canonical.get("plenora.geometry.crs_id").map(String::as_str),
            Some(CRS)
        );
        assert_eq!(
            out_schema
                .metadata()
                .get("plenora.contract.version")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn pair_passthrough_without_crs_declaration_emits_missing() {
        // R4.6.3: nessuna dichiarazione CRS sul campo propagato →
        // `crs_resolution = missing` senza chiavi CRS (mai un CRS inventato);
        // le celle non sono ricodificate → dimensions `unknown` (R3.4).
        let left = side_envelope_with_metadata(
            geoarrow_field_metadata(),
            HashMap::new(),
            &[Some(&point_wkb(0.5, 0.5))],
        );
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0))]);
        let schema = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Within, 1, 1)
        };
        let output = run_pair(&schema, &left, &right).expect("within");
        let (out_schema, _) = decode_output(&output);
        let field = out_schema
            .column_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column")
            .1;
        let canonical = canonical_block(field);
        assert_eq!(
            canonical
                .get("plenora.geometry.crs_resolution")
                .map(String::as_str),
            Some("missing")
        );
        assert_eq!(
            canonical
                .get("plenora.geometry.dimensions")
                .map(String::as_str),
            Some("unknown")
        );
        assert!(!canonical.contains_key("plenora.geometry.crs_id"));
        assert!(!canonical.contains_key("plenora.geometry.crs_definition"));
        assert!(!canonical.contains_key("plenora.geometry.axis_order"));
        assert!(!canonical.contains_key("plenora.geometry.srid"));
    }

    #[test]
    fn pair_passthrough_propagates_declared_unresolved_unchanged() {
        // R2.4/R4.6.4: un campo che porta gia' il blocco canonico (output di
        // una pipeline v4) lo conserva INVARIATO — il trasporto non
        // interpreta le chiavi canoniche.
        let mut field_metadata = geoarrow_field_metadata();
        field_metadata.insert(
            GEO_METADATA_KEY.to_owned(),
            r#"{"crs":"EPSG:4326","dimensions":"xy"}"#.to_owned(),
        );
        field_metadata.insert("plenora.geometry.dimensions".to_owned(), "xy".to_owned());
        field_metadata.insert(
            "plenora.geometry.crs_resolution".to_owned(),
            "declared_unresolved".to_owned(),
        );
        field_metadata.insert("plenora.geometry.crs_id".to_owned(), "EPSG:4326".to_owned());
        field_metadata.insert(
            "plenora.geometry.crs_definition".to_owned(),
            "LOCAL_CS[\"fixture\"]".to_owned(),
        );
        field_metadata.insert(
            "plenora.geometry.crs_definition_format".to_owned(),
            "wkt".to_owned(),
        );
        field_metadata.insert(
            "plenora.geometry.axis_order".to_owned(),
            "lon_lat".to_owned(),
        );
        field_metadata.insert("plenora.geometry.encoding".to_owned(), "wkb".to_owned());
        field_metadata.insert(
            "plenora.geometry.types_declaration".to_owned(),
            "exact".to_owned(),
        );
        field_metadata.insert("plenora.geometry.types".to_owned(), "point".to_owned());
        field_metadata.insert("plenora.geometry.srid".to_owned(), "4326".to_owned());
        let expected = field_metadata.clone();
        let schema_metadata =
            HashMap::from([("plenora.contract.version".to_owned(), "1".to_owned())]);
        let left = side_envelope_with_metadata(
            field_metadata,
            schema_metadata,
            &[Some(&point_wkb(0.5, 0.5))],
        );
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0))]);
        let schema = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Within, 1, 1)
        };
        let output = run_pair(&schema, &left, &right).expect("within");
        let (out_schema, _) = decode_output(&output);
        let field = out_schema
            .column_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column")
            .1;
        assert_eq!(field.metadata(), &expected);
        assert_eq!(
            out_schema
                .metadata()
                .get("plenora.contract.version")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn pair_passthrough_does_not_attach_observed_types_to_unresolved_declaration() {
        let mut field_metadata = geoarrow_field_metadata();
        field_metadata.insert(
            "plenora.geometry.types_declaration".to_owned(),
            "unresolved".to_owned(),
        );
        let left = side_envelope_with_metadata(
            field_metadata,
            HashMap::new(),
            &[Some(&point_wkb(0.5, 0.5))],
        );
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0))]);
        let schema = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Within, 1, 1)
        };

        let output = run_pair(&schema, &left, &right).expect("within");
        let (out_schema, _) = decode_output(&output);
        let field = out_schema
            .column_with_name(DEFAULT_GEOMETRY_COLUMN)
            .expect("geometry column")
            .1;

        assert_eq!(
            field
                .metadata()
                .get("plenora.geometry.types_declaration")
                .map(String::as_str),
            Some("unresolved")
        );
        assert!(!field.metadata().contains_key("plenora.geometry.types"));
    }

    #[test]
    fn pair_output_rejects_divergent_contract_version() {
        // R2.6/R2.5: una versione di protocollo diversa da quella corrente e'
        // un errore esplicito, mai una sovrascrittura silenziosa.
        let left = side_envelope_with_metadata(
            geoarrow_field_metadata(),
            HashMap::from([("plenora.contract.version".to_owned(), "2".to_owned())]),
            &[Some(&point_wkb(0.5, 0.5))],
        );
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0))]);
        let schema = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Within, 1, 1)
        };
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::Arrow(..))
        ));
    }

    #[test]
    fn sjoin_lineage_output_has_no_canonical_keys() {
        // Output derivato senza geometrie (indici di coppia): nessuna chiave
        // canonica e nessuna versione di protocollo (R2.5).
        let left = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0))]);
        let right = side_envelope(&[Some(&shifted_square_wkb(1.0, 1.0, 2.0))]);
        let schema = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::SJoin, 1, 1)
        };
        let output = run_pair(&schema, &left, &right).expect("sjoin");
        let (out_schema, out_batches) = decode_output(&output);
        assert!(out_schema.metadata().is_empty());
        assert!(out_schema.fields().iter().all(|field| {
            field
                .metadata()
                .keys()
                .all(|key| !key.starts_with("plenora."))
        }));
        assert!(out_batches[0].schema().metadata().is_empty());
    }

    #[test]
    fn backend_free_operations_roundtrip_with_null_preservation() {
        let square = square_wkb(2.0);
        let cases: [(ArrowOperation, TransformArrowSchema); 12] = [
            (
                ArrowOperation::Centroid,
                arrow_schema(2, ArrowOperation::Centroid),
            ),
            (
                ArrowOperation::ConvexHull,
                arrow_schema(2, ArrowOperation::ConvexHull),
            ),
            (
                ArrowOperation::Envelope,
                arrow_schema(2, ArrowOperation::Envelope),
            ),
            (
                ArrowOperation::Buffer,
                TransformArrowSchema {
                    distance: Some(0.5),
                    ..arrow_schema(2, ArrowOperation::Buffer)
                },
            ),
            (
                ArrowOperation::Simplify,
                TransformArrowSchema {
                    tolerance: Some(0.1),
                    ..arrow_schema(2, ArrowOperation::Simplify)
                },
            ),
            (
                ArrowOperation::Boundary,
                arrow_schema(2, ArrowOperation::Boundary),
            ),
            (
                ArrowOperation::PointOnSurface,
                arrow_schema(2, ArrowOperation::PointOnSurface),
            ),
            (ArrowOperation::Area, arrow_schema(2, ArrowOperation::Area)),
            (
                ArrowOperation::Length,
                arrow_schema(2, ArrowOperation::Length),
            ),
            (
                ArrowOperation::Perimeter,
                arrow_schema(2, ArrowOperation::Perimeter),
            ),
            (
                ArrowOperation::VertexCount,
                arrow_schema(2, ArrowOperation::VertexCount),
            ),
            (
                ArrowOperation::ToWkt,
                arrow_schema(2, ArrowOperation::ToWkt),
            ),
        ];
        for (operation, schema) in cases {
            let (fixture_schema, batch) = fixture_batch(&[Some(&square), None]);
            let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
            let output = run(&schema, &input)
                .unwrap_or_else(|error| panic!("{} fallita: {error}", operation.name()));
            let (out_schema, out_batches) = decode_output(&output);
            let batch = &out_batches[0];
            assert_eq!(batch.num_rows(), 2, "{}", operation.name());

            if operation.produces_geometry() {
                let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
                let cells = batch
                    .column(index)
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .expect("Binary");
                assert!(!cells.is_null(0), "{}", operation.name());
                assert!(cells.is_null(1), "{}", operation.name());
                geometry_from_wkb(cells.value(0)).unwrap_or_else(|error| {
                    panic!("{} output non valido: {error}", operation.name())
                });
            } else {
                let column_name = match operation {
                    ArrowOperation::ToWkt => WKT_COLUMN,
                    _ => operation.name(),
                };
                let index = out_schema.index_of(column_name).unwrap();
                let column = batch.column(index);
                assert!(!column.is_null(0), "{}", operation.name());
                assert!(column.is_null(1), "{}", operation.name());
            }
        }
    }

    #[test]
    fn buffer_honours_distance_cap_and_validates_parameters() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        for cap in [BufferCap::Round, BufferCap::Flat, BufferCap::Square] {
            let schema = TransformArrowSchema {
                distance: Some(1.0),
                cap: Some(cap),
                ..arrow_schema(1, ArrowOperation::Buffer)
            };
            let output = run(&schema, &input).expect("buffer");
            let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
            let cells = batch
                .column(index)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let buffered = geometry_from_wkb(cells.value(0)).expect("decode");
            let area = buffered.unsigned_area();
            // buffer(1) di un quadrato 2x2: fra quadrato espanso (16) e cerchio.
            assert!(area > 8.0 && area <= 16.0, "cap {cap:?}: area {area}");
        }

        let missing = arrow_schema(1, ArrowOperation::Buffer);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "distance",
                ..
            })
        ));

        let nan = TransformArrowSchema {
            distance: Some(f64::NAN),
            ..arrow_schema(1, ArrowOperation::Buffer)
        };
        assert!(matches!(
            run(&nan, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "distance",
                ..
            })
        ));

        let unexpected = TransformArrowSchema {
            distance: Some(1.0),
            tolerance: Some(0.1),
            ..arrow_schema(1, ArrowOperation::Buffer)
        };
        assert!(matches!(
            run(&unexpected, &input),
            Err(ArrowTransportError::UnexpectedParameter {
                name: "tolerance",
                ..
            })
        ));
    }

    #[test]
    fn simplify_honours_tolerance_policy_and_validates_parameters() {
        let mut jittered = vec![1_u8];
        jittered.extend_from_slice(&2_u32.to_le_bytes());
        jittered.extend_from_slice(&6_u32.to_le_bytes());
        for (x, y) in [
            (0.0_f64, 0.0_f64),
            (1.0, 0.01),
            (2.0, -0.01),
            (3.0, 0.01),
            (4.0, -0.01),
            (5.0, 0.0),
        ] {
            jittered.extend_from_slice(&x.to_le_bytes());
            jittered.extend_from_slice(&y.to_le_bytes());
        }
        let (fixture_schema, batch) = fixture_batch(&[Some(&jittered)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        for policy in [
            SimplifyPolicyParam::DouglasPeucker,
            SimplifyPolicyParam::PreserveTopology,
        ] {
            let schema = TransformArrowSchema {
                tolerance: Some(0.5),
                simplify_policy: Some(policy),
                ..arrow_schema(1, ArrowOperation::Simplify)
            };
            let output = run(&schema, &input).expect("simplify");
            let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
            let cells = batch
                .column(index)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let simplified = geometry_from_wkb(cells.value(0)).expect("decode");
            assert_eq!(simplified.coords_count(), 2, "policy {policy:?}");
        }

        let missing = arrow_schema(1, ArrowOperation::Simplify);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "tolerance",
                ..
            })
        ));

        let negative = TransformArrowSchema {
            tolerance: Some(-1.0),
            ..arrow_schema(1, ArrowOperation::Simplify)
        };
        assert!(matches!(
            run(&negative, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "tolerance",
                ..
            })
        ));
    }

    #[test]
    fn boundary_and_point_on_surface_produce_expected_geometry_types() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        let output = run(&arrow_schema(1, ArrowOperation::Boundary), &input).expect("boundary");
        let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
        let cells = batch
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(matches!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::MultiLineString(_)
        ));

        let output = run(&arrow_schema(1, ArrowOperation::PointOnSurface), &input)
            .expect("point_on_surface");
        let (_, batch, index) = single_cell_output(&output, DEFAULT_GEOMETRY_COLUMN);
        let cells = batch
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let point = geometry_from_wkb(cells.value(0)).unwrap();
        let Geometry::Point(point) = point else {
            panic!("point_on_surface deve produrre un Point: {point:?}")
        };
        assert!(point.x() > 0.0 && point.x() < 2.0);
        assert!(point.y() > 0.0 && point.y() < 2.0);
    }

    #[test]
    fn length_perimeter_vertex_count_bounds_and_wkt_are_exact() {
        let line = line_wkb();
        let (fixture_schema, batch) = fixture_batch(&[Some(&line), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        for (operation, expected) in [
            (ArrowOperation::Length, 7.0),
            (ArrowOperation::Perimeter, 7.0),
        ] {
            let output = run(&arrow_schema(2, operation), &input)
                .unwrap_or_else(|_| panic!("{}", operation.name()));
            let (_, batch, index) = single_cell_output(&output, operation.name());
            let values = batch
                .column(index)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            assert_eq!(values.value(0), expected, "{}", operation.name());
            assert!(values.is_null(1));
        }

        let output =
            run(&arrow_schema(2, ArrowOperation::VertexCount), &input).expect("vertex_count");
        let (out_schema, batch, index) = single_cell_output(&output, "vertex_count");
        assert_eq!(out_schema.field(index).data_type(), &DataType::UInt64);
        let values = batch
            .column(index)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(values.value(0), 3);
        assert!(values.is_null(1));

        let output = run(&arrow_schema(2, ArrowOperation::Bounds), &input).expect("bounds");
        let (out_schema, batches) = decode_output(&output);
        let expected_bounds = [
            ("geometry_minx", 0.0),
            ("geometry_miny", 0.0),
            ("geometry_maxx", 3.0),
            ("geometry_maxy", 4.0),
        ];
        for (name, expected) in expected_bounds {
            let index = out_schema.index_of(name).expect("colonna bounds");
            let values = batches[0]
                .column(index)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            assert_eq!(values.value(0), expected, "{name}");
            assert!(values.is_null(1));
        }

        let output = run(&arrow_schema(2, ArrowOperation::ToWkt), &input).expect("to_wkt");
        let (out_schema, batch, index) = single_cell_output(&output, WKT_COLUMN);
        assert_eq!(out_schema.field(index).data_type(), &DataType::Utf8);
        let values = batch
            .column(index)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        // Adattamento Fase 1: il crate `wkt` non e' una dipendenza di
        // plenora-engine (nel sorgente il WKT veniva ri-parsato con
        // `wkt::TryFromWkt` e confrontato con la geometria attesa); qui il
        // confronto usa il kernel `to_wkt` come riferimento canonico.
        let expected_wkt = to_wkt(&geometry_from_wkb(&line).unwrap()).expect("wkt atteso");
        assert_eq!(values.value(0), expected_wkt);
        assert!(values.is_null(1));
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn make_valid_repairs_bowtie_and_preserves_valid_geometries() {
        let mut bowtie = vec![1_u8];
        bowtie.extend_from_slice(&3_u32.to_le_bytes());
        bowtie.extend_from_slice(&1_u32.to_le_bytes());
        bowtie.extend_from_slice(&5_u32.to_le_bytes());
        for (x, y) in [
            (0.0_f64, 0.0_f64),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0),
            (0.0, 0.0),
        ] {
            bowtie.extend_from_slice(&x.to_le_bytes());
            bowtie.extend_from_slice(&y.to_le_bytes());
        }
        assert!(geometry_from_wkb(&bowtie).is_err());

        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&bowtie), None, Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(3, ArrowOperation::MakeValid), &input).expect("make_valid");
        let (out_schema, out_batches) = decode_output(&output);
        let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
        let cells = out_batches[0]
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let repaired = geometry_from_wkb(cells.value(0)).expect("riparata");
        assert!((repaired.unsigned_area() - 2.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert_eq!(cells.value(2), square.as_slice());
    }

    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn make_valid_without_geos_fails_closed() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::MakeValid), &input),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "geos-backend",
                ..
            })
        ));
    }

    #[cfg(feature = "proj-backend")]
    #[test]
    fn reproject_transforms_coordinates_and_stamps_target_crs() {
        let mut point = vec![1_u8, 1, 0, 0, 0];
        point.extend_from_slice(&12.0_f64.to_le_bytes());
        point.extend_from_slice(&41.0_f64.to_le_bytes());
        let (fixture_schema, batch) = fixture_batch(&[Some(&point), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));

        let schema = TransformArrowSchema {
            crs: Some("EPSG:4326".to_owned()),
            target_crs: Some(CRS.to_owned()),
            ..arrow_schema(2, ArrowOperation::Reproject)
        };
        let output = run(&schema, &input).expect("reproject");
        let (out_schema, out_batches) = decode_output(&output);
        let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
        let geo: serde_json::Value = serde_json::from_str(
            out_schema
                .field(index)
                .metadata()
                .get(GEO_METADATA_KEY)
                .expect("geo metadata"),
        )
        .unwrap();
        assert_eq!(
            geo.get("crs").and_then(serde_json::Value::as_str),
            Some(CRS)
        );
        let cells = out_batches[0]
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let reprojected = geometry_from_wkb(cells.value(0)).unwrap();
        let Geometry::Point(point) = reprojected else {
            panic!("atteso Point: {reprojected:?}")
        };
        // EPSG:3857 di (12E, 41N) calcolato con PROJ.
        assert!((point.x() - 1_335_833.889_5).abs() < 0.01);
        assert!((point.y() - 5_012_341.663_8).abs() < 0.01);
        assert!(cells.is_null(1));

        let missing_target = TransformArrowSchema {
            crs: Some("EPSG:4326".to_owned()),
            ..arrow_schema(2, ArrowOperation::Reproject)
        };
        assert!(matches!(
            run(&missing_target, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "target_crs",
                ..
            })
        ));
    }

    #[cfg(not(feature = "proj-backend"))]
    #[test]
    fn reproject_without_proj_fails_closed() {
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let schema = TransformArrowSchema {
            target_crs: Some("EPSG:32632".to_owned()),
            ..arrow_schema(1, ArrowOperation::Reproject)
        };
        assert!(matches!(
            run(&schema, &input),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "proj-backend",
                ..
            })
        ));
    }

    #[test]
    fn multiple_batches_are_preserved_in_output() {
        let square = square_wkb(1.0);
        let (schema, first) = fixture_batch(&[Some(&square)]);
        let (_, second) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, &[first, second]);
        let output = run(&arrow_schema(2, ArrowOperation::Envelope), &input).expect("transform");
        let (_, batches) = decode_output(&output);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].num_rows(), 1);
        assert_eq!(batches[1].num_rows(), 1);
    }

    #[test]
    fn single_byte_corruption_is_detected_by_checksum() {
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let mut input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let flip = input.len() / 2;
        input[flip] ^= 0x01;
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &input),
            Err(ArrowTransportError::ChecksumMismatch)
        ));
    }

    #[test]
    fn truncation_trailing_bytes_and_bad_magic_fail_closed() {
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        for cut in [1_usize, 8, 20, 40] {
            let truncated = &input[..input.len() - cut];
            assert!(
                run(&arrow_schema(1, ArrowOperation::Centroid), truncated).is_err(),
                "cut={cut}"
            );
        }

        let mut extra = input.clone();
        extra.push(0);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &extra),
            Err(ArrowTransportError::TrailingBytes)
        ));

        let mut bad_magic = input;
        bad_magic[0] = b'X';
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &bad_magic),
            Err(ArrowTransportError::InvalidMagic)
        ));

        let mut bad_trailer =
            envelope_bytes(&schema, std::slice::from_ref(&fixture_batch(&[None]).1));
        let trailer_start = bad_trailer.len() - 40;
        bad_trailer[trailer_start] ^= 0x01;
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &bad_trailer),
            Err(ArrowTransportError::InvalidTrailer)
        ));
    }

    #[test]
    fn row_count_schema_version_and_resource_limits_fail_closed() {
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        assert!(matches!(
            run(&arrow_schema(2, ArrowOperation::Centroid), &input),
            Err(ArrowTransportError::RowCountMismatch {
                schema: 2,
                stream: 1
            })
        ));

        let mut wrong_version = arrow_schema(1, ArrowOperation::Centroid);
        wrong_version.schema_version = 2;
        assert!(matches!(
            run(&wrong_version, &input),
            Err(ArrowTransportError::UnsupportedSchemaVersion(2))
        ));

        let too_many = arrow_schema(MAX_ROWS + 1, ArrowOperation::Centroid);
        assert!(matches!(
            run(&too_many, &input),
            Err(ArrowTransportError::TooManyRows(_))
        ));

        let missing_crs = TransformArrowSchema {
            crs: None,
            ..arrow_schema(1, ArrowOperation::Centroid)
        };
        assert!(matches!(
            run(&missing_crs, &input),
            Err(ArrowTransportError::CrsRequired)
        ));
    }

    #[test]
    fn column_and_batch_limits_fail_closed() {
        let wide_fields: Vec<Field> = (0..=MAX_COLUMNS)
            .map(|index| Field::new(format!("col{index}"), DataType::Int64, true))
            .collect();
        let wide_schema = Arc::new(Schema::new(wide_fields));
        let payload = encode_ipc(&wide_schema, &[]).expect("encode wide");
        assert!(matches!(
            decode_ipc(&payload),
            Err(ArrowTransportError::TooManyColumns(_))
        ));

        let (schema, batch) = fixture_batch(&[None]);
        let batches = vec![batch; MAX_BATCHES + 1];
        assert!(matches!(
            encode_ipc(&schema, &batches),
            Err(ArrowTransportError::TooManyBatches(_))
        ));
    }

    #[test]
    fn oversized_wkb_cell_fails_before_validation() {
        // MAX_CELL_BYTES e' una costante da 64 MiB: entra in usize su
        // ogni target supportato; la conversione e' totale per contratto.
        let oversized =
            vec![0_u8; usize::try_from(MAX_CELL_BYTES).expect("limite celle entro usize") + 1];
        let (schema, batch) = fixture_batch(&[Some(&oversized)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &input)
                .expect_err("cella oltre il limite")
                .source_error(),
            ArrowTransportError::CellTooLarge(_)
        ));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Area), &input)
                .expect_err("cella oltre il limite")
                .source_error(),
            ArrowTransportError::CellTooLarge(_)
        ));
    }

    #[test]
    fn geometry_column_contract_is_fail_closed() {
        let (schema, batch) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        let renamed = TransformArrowSchema {
            geometry_column: Some("assente".to_owned()),
            ..arrow_schema(1, ArrowOperation::Centroid)
        };
        assert!(matches!(
            run(&renamed, &input),
            Err(ArrowTransportError::MissingGeometryColumn(_))
        ));

        let wrong_type_schema = Arc::new(Schema::new(vec![Field::new(
            DEFAULT_GEOMETRY_COLUMN,
            DataType::Int64,
            true,
        )]));
        let wrong_type_batch = RecordBatch::try_new(
            wrong_type_schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1_i64]))],
        )
        .unwrap();
        let wrong_type = envelope_bytes(&wrong_type_schema, &[wrong_type_batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &wrong_type),
            Err(ArrowTransportError::GeometryColumnNotBinary { .. })
        ));

        let no_metadata_schema = Arc::new(Schema::new(vec![Field::new(
            DEFAULT_GEOMETRY_COLUMN,
            DataType::Binary,
            true,
        )]));
        let no_metadata_batch = RecordBatch::try_new(
            no_metadata_schema.clone(),
            vec![Arc::new(BinaryArray::from_iter([None::<&[u8]>]))],
        )
        .unwrap();
        let no_metadata = envelope_bytes(&no_metadata_schema, &[no_metadata_batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Centroid), &no_metadata),
            Err(ArrowTransportError::MissingGeoArrowMetadata(_))
        ));

        let invalid_wkb = vec![0xde_u8, 0xad];
        let (schema, batch) = fixture_batch(&[Some(&invalid_wkb)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Area), &input)
                .expect_err("WKB invalido")
                .source_error(),
            ArrowTransportError::Geometry(_)
        ));
    }

    /// Regressione fuzz: `arrow-ipc` va in panico decodificando lo schema, e
    /// `decode_ipc` deve restituire un errore invece di far abortire il
    /// processo.
    ///
    /// Questo test e' l'UNICA copertura possibile della barriera. Il fuzz
    /// target `arrow_transform` non puo' verificarla: `libfuzzer-sys` installa
    /// un hook di panico che chiama `std::process::abort()` prima che
    /// l'unwinding cominci (libfuzzer-sys 0.4.10, src/lib.rs:92-95), proprio
    /// perche' un `catch_unwind` nel codice sotto test nasconderebbe i difetti
    /// al fuzzer. Quel target resta quindi in quarantena e restera' rosso
    /// anche a barriera funzionante: non e' un difetto della mitigazione, e'
    /// lo strumento progettato per non farsi ingannare da essa.
    #[test]
    fn ipc_decode_converte_il_panico_di_arrow_in_errore() {
        // 81 byte trovati dalla campagna schedulata del 2026-08-07, artefatto
        // crash-c20d19d3e3323f54d3831c09d611143c5d8f82c1. Superano il framing
        // e fanno arrivare ad `arrow-ipc` uno schema FlatBuffer con un valore
        // di enum che `convert::fb_to_schema` non riconosce: la funzione ha
        // venti `panic!`/`unimplemented!` e i reader la chiamano sempre.
        let payload = [
            0x2c, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x04, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x3d, 0x08, 0x00, 0x22, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00,
            0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x60,
            0x00, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x08,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        // L'hook di panico del processo stampa comunque su stderr: lo
        // silenziamo per la durata del test, altrimenti l'output della suite
        // sembra un fallimento. Ripristinato subito dopo.
        let precedente = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let esito = decode_ipc(&payload);
        std::panic::set_hook(precedente);

        assert!(
            matches!(esito, Err(ArrowTransportError::ArrowPanic(_))),
            "atteso ArrowPanic, ottenuto {esito:?}"
        );
    }

    #[test]
    fn ipc_decode_rejects_oversized_metadata_and_truncation_without_oom() {
        // Regressione fuzz (OOM): 4 byte che dichiarano ~709 MiB di metadati
        // in formato legacy; prima della pre-validazione arrow-rs allocava
        // quanto dichiarato.
        let oom_input = [0x5b, 0x74, 0x32, 0x2a];
        assert!(matches!(
            decode_ipc(&oom_input),
            Err(ArrowTransportError::IpcMetadataTooLarge(707_949_659))
        ));
        // continuazione valida ma metadati troncati.
        let truncated = [0xff, 0xff, 0xff, 0xff, 0x10, 0x00];
        assert!(matches!(
            decode_ipc(&truncated),
            Err(ArrowTransportError::IpcTruncated)
        ));
        assert!(matches!(
            decode_ipc(&[]),
            Err(ArrowTransportError::IpcTruncated)
        ));
        // metadati oltre il tetto assoluto anche con continuazione moderna.
        // MAX_IPC_METADATA_BYTES e' una costante da 16 MiB: entra in u32;
        // la conversione e' totale per contratto.
        let declared = u32::try_from(MAX_IPC_METADATA_BYTES).expect("tetto metadati entro u32") + 8;
        let mut oversized = vec![0xff, 0xff, 0xff, 0xff];
        oversized.extend_from_slice(&declared.to_le_bytes());
        oversized.extend_from_slice(&[0; 16]);
        assert!(matches!(
            decode_ipc(&oversized),
            Err(ArrowTransportError::IpcMetadataTooLarge(_))
        ));
        // un batch valido continua a decodificare (framing reale coperto dai
        // roundtrip degli altri test).
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let payload = encode_ipc(&schema, std::slice::from_ref(&batch)).expect("encode");
        assert!(decode_ipc(&payload).is_ok());
    }

    #[test]
    fn unknown_schema_field_is_rejected_and_operation_params_parse() {
        let body = br#"{"schema_version":3,"operation":"centroid","row_count":1,"crs":"EPSG:3857","sconosciuto":true}"#;
        assert!(serde_json::from_slice::<TransformArrowSchema>(body).is_err());

        let minimal = br#"{"schema_version":3,"operation":"area","row_count":1,"crs":"EPSG:3857"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(minimal).expect("schema");
        assert_eq!(schema.geometry_column(), DEFAULT_GEOMETRY_COLUMN);
        assert_eq!(schema.operation, ArrowOperation::Area);

        let buffer = br#"{"schema_version":3,"operation":"buffer","row_count":1,"crs":"EPSG:3857","distance":2.5,"cap":"flat"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(buffer).expect("buffer schema");
        assert_eq!(schema.distance, Some(2.5));
        assert_eq!(schema.cap, Some(BufferCap::Flat));
        schema.validate_parameters().expect("parametri validi");

        let simplify = br#"{"schema_version":3,"operation":"simplify","row_count":1,"crs":"EPSG:3857","tolerance":0.1,"simplify_policy":"preserve_topology"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(simplify).expect("simplify");
        assert_eq!(
            schema.simplify_policy,
            Some(SimplifyPolicyParam::PreserveTopology)
        );

        let reproject = br#"{"schema_version":3,"operation":"reproject","row_count":1,"crs":"EPSG:4326","target_crs":"EPSG:3857"}"#;
        let schema: TransformArrowSchema = serde_json::from_slice(reproject).expect("reproject");
        schema.validate_parameters().expect("parametri validi");
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
            Err(ArrowTransportError::CrsRequired)
        ));
        let oversized = "X".repeat(MAX_CRS_DEFINITION_BYTES + 1);
        assert!(matches!(
            geo_metadata_json(&oversized),
            Err(ArrowTransportError::CrsTooLarge)
        ));
    }

    #[test]
    fn geo_metadata_json_is_byte_identical_to_arrow_adapter() {
        // Unificazione B1.1: il trasporto delega l'assemblaggio JSON ad
        // `arrow_adapter`; l'output deve essere identico byte-per-byte.
        for crs in [CRS, r#"{"type":"ProjectedCRS","name":"demo"}"#] {
            assert_eq!(
                geo_metadata_json(crs).expect("transport"),
                plenora_kernels_geo::arrow_adapter::geo_metadata_json(crs).expect("adapter")
            );
        }
        // Il campo di output dichiara anche la dimensionalita' (B1.1).
        let field = geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("field");
        let geo: serde_json::Value = serde_json::from_str(
            field
                .metadata()
                .get(GEO_METADATA_KEY)
                .expect("geo metadata"),
        )
        .expect("geo JSON");
        assert_eq!(
            geo.get("dimensions").and_then(serde_json::Value::as_str),
            Some("xy")
        );
        assert_eq!(
            field.metadata().get(GEO_METADATA_KEY).map(String::as_str),
            plenora_kernels_geo::arrow_adapter::geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS)
                .expect("adapter field")
                .metadata()
                .get(GEO_METADATA_KEY)
                .map(String::as_str)
        );
    }

    #[test]
    fn writer_rejects_payload_beyond_declared_length() {
        let mut writer = EnvelopeWriter::new(Vec::new(), 4).expect("writer");
        writer.write_payload(b"ab").expect("chunk");
        assert!(matches!(
            writer.write_payload(b"cde"),
            Err(ArrowTransportError::StreamTooLarge)
        ));
        assert!(matches!(
            writer.finish(),
            Err(ArrowTransportError::PayloadLengthMismatch {
                declared: 4,
                written: 2
            })
        ));
    }

    #[test]
    fn ipc_roundtrip_through_cursor_io() {
        let square = square_wkb(3.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let mut output = Vec::new();
        transform_arrow(
            Cursor::new(input),
            Cursor::new(&mut output),
            &arrow_schema(1, ArrowOperation::ConvexHull),
        )
        .expect("transform");
        let (_, batches) = decode_output(&output);
        let geometry = geometry_from_wkb(
            batches[0]
                .column(3)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap()
                .value(0),
        )
        .unwrap();
        assert!(geometry.unsigned_area() > 0.0);
    }

    fn multipoint_wkb() -> Vec<u8> {
        Geometry::MultiPoint(geo::MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 2.0),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("fixture WKB")
    }

    #[test]
    fn explode_expands_rows_with_lineage_and_replicated_attributes() {
        let multi = multipoint_wkb();
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&multi), None, Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let explode_schema = TransformArrowSchema {
            max_output_rows: Some(16),
            ..arrow_schema(3, ArrowOperation::Explode)
        };
        let output = run(&explode_schema, &input).expect("explode");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches.len(), 1);
        let batch = &out_batches[0];
        // 3 punti dal MultiPoint + 1 riga dal Polygon semplice; null senza figli.
        assert_eq!(batch.num_rows(), 4);

        let parents = batch
            .column(out_schema.index_of(PARENT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .expect("UInt64");
        assert_eq!(parents.values(), &[0, 0, 0, 2]);
        assert!(!parents.is_nullable());

        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64");
        assert_eq!(ids.values(), &[0, 0, 0, 2]);
        let labels = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8");
        assert_eq!(labels.value(3), "riga-2");

        let cells = batch
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .expect("Binary");
        for row in 0..3 {
            let Geometry::Point(point) = geometry_from_wkb(cells.value(row)).unwrap() else {
                panic!("componente MultiPoint deve essere Point")
            };
            // `row` e' l'indice del loop 0..3: esatto in f64.
            #[allow(clippy::cast_precision_loss)]
            let coordinate = row as f64;
            assert_eq!(point, Point::new(coordinate, coordinate));
        }
        assert!(matches!(
            geometry_from_wkb(cells.value(3)).unwrap(),
            Geometry::Polygon(_)
        ));
    }

    #[test]
    fn explode_enforces_max_output_rows_incrementally() {
        let multi = multipoint_wkb();
        let (schema, batch) = fixture_batch(&[Some(&multi)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        let missing = arrow_schema(1, ArrowOperation::Explode);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "max_output_rows",
                ..
            })
        ));

        let too_small = TransformArrowSchema {
            max_output_rows: Some(2),
            ..arrow_schema(1, ArrowOperation::Explode)
        };
        assert!(matches!(
            run(&too_small, &input),
            Err(ArrowTransportError::OutputRowsExceeded {
                actual: 3,
                limit: 2
            })
        ));
    }

    #[test]
    fn dissolve_merges_polygons_and_rejects_other_types() {
        let mut shifted = vec![1_u8];
        shifted.extend_from_slice(&3_u32.to_le_bytes());
        shifted.extend_from_slice(&1_u32.to_le_bytes());
        shifted.extend_from_slice(&5_u32.to_le_bytes());
        for (x, y) in [
            (1.0_f64, 0.0_f64),
            (3.0, 0.0),
            (3.0, 2.0),
            (1.0, 2.0),
            (1.0, 0.0),
        ] {
            shifted.extend_from_slice(&x.to_le_bytes());
            shifted.extend_from_slice(&y.to_le_bytes());
        }
        let square = square_wkb(2.0);
        let (schema, batch) = fixture_batch(&[Some(&square), None, Some(&shifted)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(3, ArrowOperation::Dissolve), &input).expect("dissolve");
        let (out_schema, out_batches) = decode_output(&output);
        // una sola riga, solo colonna geometria, attributi non propagati.
        assert_eq!(out_schema.fields().len(), 1);
        assert_eq!(out_batches[0].num_rows(), 1);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let dissolved = geometry_from_wkb(cells.value(0)).expect("decode");
        assert!((dissolved.unsigned_area() - 6.0).abs() < 1e-12);

        // solo null: una riga con geometria null.
        let (schema, batch) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(1, ArrowOperation::Dissolve), &input).expect("dissolve");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(cells.is_null(0));

        // input non poligonale: rifiutato dal kernel.
        let line = line_wkb();
        let (schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Dissolve), &input),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    #[test]
    fn line_and_polygon_builder_use_input_order_and_skip_nulls() {
        let points: Vec<Vec<u8>> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .into_iter()
            .map(|(x, y)| {
                Geometry::Point(Point::new(x, y))
                    .to_wkb(CoordDimensions::xy())
                    .unwrap()
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = vec![
            Some(&points[0]),
            None,
            Some(&points[1]),
            Some(&points[2]),
            Some(&points[3]),
        ];
        let (schema, batch) = fixture_batch(&refs);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));

        let output = run(&arrow_schema(5, ArrowOperation::LineBuilder), &input).expect("line");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let Geometry::LineString(line) = geometry_from_wkb(cells.value(0)).unwrap() else {
            panic!("line_builder deve produrre LineString")
        };
        assert_eq!(line.coords_count(), 4);

        let output =
            run(&arrow_schema(5, ArrowOperation::PolygonBuilder), &input).expect("polygon");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let polygon = geometry_from_wkb(cells.value(0)).unwrap();
        assert!((polygon.unsigned_area() - 1.0).abs() < 1e-12);

        // punti insufficienti: riga null, non errore.
        let (schema, batch) = fixture_batch(&[Some(&points[0]), None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(2, ArrowOperation::LineBuilder), &input).expect("line");
        let (_, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(cells.is_null(0));

        // input non puntuale: fail-closed.
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::LineBuilder), &input),
            Err(ArrowTransportError::Construction(_))
        ));

        // ordine auto-intersecante: il kernel rifiuta il poligono invalido.
        let bowtie_points: Vec<Vec<u8>> = [(0.0, 0.0), (1.0, 1.0), (0.0, 1.0), (1.0, 0.0)]
            .into_iter()
            .map(|(x, y)| {
                Geometry::Point(Point::new(x, y))
                    .to_wkb(CoordDimensions::xy())
                    .unwrap()
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = bowtie_points.iter().map(|p| Some(p.as_slice())).collect();
        let (schema, batch) = fixture_batch(&refs);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(4, ArrowOperation::PolygonBuilder), &input),
            Err(ArrowTransportError::Construction(_))
        ));
    }

    #[test]
    fn voronoi_preserves_positions_and_enforces_point_cap() {
        use geo::Intersects;

        let points: Vec<Vec<u8>> = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]
            .into_iter()
            .map(|(x, y)| {
                Geometry::Point(Point::new(x, y))
                    .to_wkb(CoordDimensions::xy())
                    .unwrap()
            })
            .collect();
        let refs: Vec<Option<&[u8]>> = vec![
            Some(&points[0]),
            None,
            Some(&points[1]),
            Some(&points[2]),
            Some(&points[3]),
        ];
        let (schema, batch) = fixture_batch(&refs);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(5, ArrowOperation::Voronoi), &input).expect("voronoi");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches[0].num_rows(), 5);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert!(cells.is_null(1));
        for (row, point) in [
            (0, &points[0]),
            (2, &points[1]),
            (3, &points[2]),
            (4, &points[3]),
        ] {
            let cell = geometry_from_wkb(cells.value(row)).expect("cella");
            let expected_point = geometry_from_wkb(point).unwrap();
            assert!(cell.intersects(&expected_point), "cella riga {row}");
        }
        // attributi preservati sulle stesse righe.
        let ids = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2, 3, 4]);

        // cap punti dal kernel.
        let capped = TransformArrowSchema {
            max_points: Some(3),
            ..arrow_schema(5, ArrowOperation::Voronoi)
        };
        assert!(matches!(
            run(&capped, &input),
            Err(ArrowTransportError::Advanced(_))
        ));

        // max_points non valido nello schema.
        let invalid = TransformArrowSchema {
            max_points: Some(1),
            ..arrow_schema(5, ArrowOperation::Voronoi)
        };
        assert!(matches!(
            run(&invalid, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_points",
                ..
            })
        ));

        // input non puntuale: fail-closed.
        let square = square_wkb(1.0);
        let (schema, batch) = fixture_batch(&[Some(&square), Some(&square)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(2, ArrowOperation::Voronoi), &input),
            Err(ArrowTransportError::Advanced(_))
        ));
    }

    fn coords_batch(xs: Vec<Option<f64>>, ys: Vec<Option<f64>>) -> (SchemaRef, RecordBatch) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("x", DataType::Float64, true),
            Field::new("y", DataType::Float64, true),
        ]));
        // righe fixture: poche per costruzione, entro i64.
        let rows = i64::try_from(xs.len()).expect("righe fixture entro i64");
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from((0..rows).collect::<Vec<i64>>())),
                Arc::new(Float64Array::from(xs)),
                Arc::new(Float64Array::from(ys)),
            ],
        )
        .expect("coords batch");
        (schema, batch)
    }

    #[test]
    fn from_coords_builds_points_without_geometry_input() {
        let (schema, batch) = coords_batch(
            vec![Some(12.0), None, Some(7.5)],
            vec![Some(41.0), Some(1.0), None],
        );
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let output =
            run(&arrow_schema(3, ArrowOperation::FromCoords), &input).expect("from_coords");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_schema.fields().len(), 4);
        let index = out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap();
        let field = out_schema.field(index);
        assert_eq!(
            field
                .metadata()
                .get(GEOARROW_EXTENSION_KEY)
                .map(String::as_str),
            Some(GEOARROW_WKB_EXTENSION)
        );
        let cells = out_batches[0]
            .column(index)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::Point(Point::new(12.0, 41.0))
        );
        assert!(cells.is_null(1));
        assert!(cells.is_null(2));
        let ids = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);

        // coordinate non finite: rifiuto fail-closed row-scoped.
        let (schema, batch) = coords_batch(vec![Some(f64::NAN)], vec![Some(0.0)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let error = run(&arrow_schema(1, ArrowOperation::FromCoords), &input)
            .expect_err("coordinata non finita");
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["geometry.non_finite_coordinate"], 1);

        // colonna assente.
        let (schema, batch) = coords_batch(vec![Some(1.0)], vec![Some(2.0)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let renamed = TransformArrowSchema {
            x_column: Some("lon".to_owned()),
            ..arrow_schema(1, ArrowOperation::FromCoords)
        };
        assert!(matches!(
            run(&renamed, &input),
            Err(ArrowTransportError::MissingColumn(_))
        ));

        // colonna non numerica.
        let bad_schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Utf8, true),
            Field::new("y", DataType::Float64, true),
        ]));
        let bad_batch = RecordBatch::try_new(
            bad_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![Some("testo")])),
                Arc::new(Float64Array::from(vec![Some(2.0)])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&bad_schema, &[bad_batch]);
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input),
            Err(ArrowTransportError::ColumnNotNumeric { .. })
        ));

        // collisione col nome geometria di output.
        let (schema, batch) = fixture_batch(&[None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input),
            Err(ArrowTransportError::OutputColumnExists(_))
        ));
    }

    #[test]
    fn one_to_one_reports_absolute_indices_and_complete_scan_across_batches() {
        // P0-2: due batch, WKB malformato in ENTRAMBI. Il trasporto deve
        // scansionare tutti i batch, aggregare con offset assoluti checked
        // e chiudere fail-closed senza pubblicare output. Copre il choke
        // point comune delle op 1:1: transform (Centroid), misura (Area),
        // bounds (Bounds, quattro colonne Float64).
        let malformed: &[u8] = &[0x01, 0x09, 0x00];
        let (schema, batch_a) = fixture_batch(&[
            Some(line_wkb().as_slice()),
            Some(malformed),
            Some(line_wkb().as_slice()),
        ]);
        let (_, batch_b) = fixture_batch(&[Some(malformed), Some(line_wkb().as_slice())]);
        let input = envelope_bytes(&schema, &[batch_a, batch_b]);
        for operation in [
            ArrowOperation::Centroid,
            ArrowOperation::Area,
            ArrowOperation::Bounds,
        ] {
            let error = run(&arrow_schema(5, operation), &input)
                .expect_err("output pubblicato nonostante righe invalide");
            let report = error
                .row_diagnostics()
                .expect("diagnostica row-scoped aggregata");
            assert_eq!(report.observed_total, 2, "{operation:?}");
            assert_eq!(report.total, Some(2), "{operation:?}");
            assert_eq!(report.counts["geometry.invalid_wkb"], 2, "{operation:?}");
            assert_eq!(
                report.completeness,
                RowDiagnosticsCompleteness::Complete,
                "{operation:?}"
            );
            let indices: Vec<u64> = report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect();
            assert_eq!(indices, vec![1, 3], "{operation:?}");
            assert!(report.validate_for_emission().is_ok(), "{operation:?}");
        }
    }

    #[test]
    fn one_to_one_invalid_only_in_later_batch_reports_absolute_index() {
        // P0-2 (controllo): primo batch pulito, WKB malformato solo nel
        // secondo -> l'indice pubblicato e' assoluto (offset del primo
        // batch applicato), nessun indice batch-locale spacciato.
        let malformed: &[u8] = &[0x01, 0x09, 0x00];
        let (schema, batch_a) =
            fixture_batch(&[Some(line_wkb().as_slice()), Some(line_wkb().as_slice())]);
        let (_, batch_b) = fixture_batch(&[Some(line_wkb().as_slice()), Some(malformed)]);
        let input = envelope_bytes(&schema, &[batch_a, batch_b]);
        let error = run(&arrow_schema(4, ArrowOperation::Centroid), &input)
            .expect_err("WKB malformato nel secondo batch");
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.total, Some(1));
        assert_eq!(report.examples[0].source_index, 3);
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Complete);
        assert!(report.validate_for_emission().is_ok());
    }

    #[test]
    fn one_to_one_late_non_row_scoped_error_preserves_partial_diagnostics() {
        // P2: il primo batch produce un rifiuto row-scoped (diagnostica gia'
        // osservata); il secondo un errore NON row-scoped (colonna geometria
        // non Binary — schema drift tra batch dello stesso stream). L'errore
        // reale deve propagare CON il report accumulato declassato a Partial
        // (knowledge limit stabile, `total` sconosciuto) e zero accepted —
        // mai la perdita silenziosa della diagnostica gia' raccolta.
        let malformed: &[u8] = &[0x01, 0x09, 0x00];
        let drift_batch = || {
            let drift_schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("label", DataType::Utf8, true),
                Field::new("weight", DataType::Float64, true),
                Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Utf8, true),
            ]));
            RecordBatch::try_new(
                drift_schema,
                vec![
                    Arc::new(Int64Array::from(vec![0_i64])),
                    Arc::new(StringArray::from(vec![Some("drift")])),
                    Arc::new(Float64Array::from(vec![Some(0.0)])),
                    Arc::new(StringArray::from(vec![Some("non-wkb")])),
                ],
            )
            .expect("drift batch")
        };
        let (schema, batch_a) = fixture_batch(&[Some(line_wkb().as_slice()), Some(malformed)]);
        let error = super::super::unary::transform_batches(
            &schema,
            &[batch_a, drift_batch()],
            &arrow_schema(3, ArrowOperation::Centroid),
        )
        .expect_err("errore non row-scoped tardivo");
        assert!(
            matches!(
                error.source_error(),
                ArrowTransportError::GeometryColumnNotBinary { .. }
            ),
            "errore reale preservato: {error:?}"
        );
        let report = error
            .row_diagnostics()
            .expect("diagnostica accumulata non persa");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Partial);
        assert_eq!(report.total, None);
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["geometry.invalid_wkb"], 1);
        assert_eq!(report.examples[0].source_index, 1);
        assert_eq!(
            report.knowledge_limits,
            Some(vec!["data_tools.processing_interrupted".to_owned()])
        );
        assert!(report.validate_for_emission().is_ok());

        // Controllo: senza diagnostica accumulata l'errore non row-scoped
        // propaga com'e', senza report (comportamento storico invariato).
        let (schema, clean) = fixture_batch(&[Some(line_wkb().as_slice())]);
        let error = super::super::unary::transform_batches(
            &schema,
            &[clean, drift_batch()],
            &arrow_schema(2, ArrowOperation::Centroid),
        )
        .expect_err("drift senza diagnostica");
        assert!(
            matches!(error, ArrowTransportError::GeometryColumnNotBinary { .. }),
            "{error:?}"
        );
        assert!(error.row_diagnostics().is_none());
    }

    #[test]
    fn from_coords_late_non_row_scoped_error_preserves_partial_diagnostics() {
        // P2 (stessa classe di one_to_one): il primo batch produce una
        // rejection row-scoped (coordinata NaN); il secondo un errore NON
        // row-scoped (colonna x non numerica — schema drift). L'errore reale
        // propaga con il report delle rejection osservate declassato a
        // Partial, knowledge limit dichiarato, zero accepted.
        let (schema, batch_a) =
            coords_batch(vec![Some(1.0), Some(f64::NAN)], vec![Some(2.0), Some(4.0)]);
        let drift_batch = || {
            let drift_schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("x", DataType::Utf8, true),
                Field::new("y", DataType::Float64, true),
            ]));
            RecordBatch::try_new(
                drift_schema,
                vec![
                    Arc::new(Int64Array::from(vec![0_i64])),
                    Arc::new(StringArray::from(vec![Some("non-numerica")])),
                    Arc::new(Float64Array::from(vec![Some(2.0)])),
                ],
            )
            .expect("drift batch")
        };
        let error = super::super::unary::transform_batches(
            &schema,
            &[batch_a, drift_batch()],
            &arrow_schema(3, ArrowOperation::FromCoords),
        )
        .expect_err("errore non row-scoped tardivo");
        assert!(
            matches!(
                error.source_error(),
                ArrowTransportError::ColumnNotNumeric { .. }
            ),
            "errore reale preservato: {error:?}"
        );
        let report = error
            .row_diagnostics()
            .expect("diagnostica accumulata non persa");
        assert_eq!(report.completeness, RowDiagnosticsCompleteness::Partial);
        assert_eq!(report.total, None);
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["geometry.non_finite_coordinate"], 1);
        assert_eq!(report.examples[0].source_index, 1);
        assert_eq!(report.examples[0].column.as_deref(), Some("x"));
        assert_eq!(
            report.knowledge_limits,
            Some(vec!["data_tools.processing_interrupted".to_owned()])
        );
        assert!(report.validate_for_emission().is_ok());

        // Controllo: senza rejection accumulate l'errore propaga com'e'.
        let (schema, clean) = coords_batch(vec![Some(1.0)], vec![Some(2.0)]);
        let error = super::super::unary::transform_batches(
            &schema,
            &[clean, drift_batch()],
            &arrow_schema(2, ArrowOperation::FromCoords),
        )
        .expect_err("drift senza rejection");
        assert!(
            matches!(error, ArrowTransportError::ColumnNotNumeric { .. }),
            "{error:?}"
        );
        assert!(error.row_diagnostics().is_none());
    }

    #[test]
    fn from_coords_reports_row_diagnostics_with_absolute_indices() {
        // Due casi mono-batch per causa, piu' un caso multi-batch omogeneo:
        // i batch di un envelope devono condividere lo schema logico.
        let (schema, first) = coords_batch(
            vec![Some(1.0), Some(f64::NAN), Some(3.0)],
            vec![Some(2.0), Some(4.0), Some(6.0)],
        );
        let input = envelope_bytes(&schema, std::slice::from_ref(&first));
        let error = run(&arrow_schema(3, ArrowOperation::FromCoords), &input)
            .expect_err("coordinata non finita");
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.total, Some(1));
        assert_eq!(report.counts["geometry.non_finite_coordinate"], 1);
        assert_eq!(report.examples[0].source_index, 1);
        assert_eq!(report.examples[0].column.as_deref(), Some("x"));
        assert!(report.validate_for_emission().is_ok());
        assert!(matches!(error, ArrowTransportError::RowDiagnostics { .. }));

        // Multi-batch omogeneo: NaN alla riga 1 del primo batch (2 righe) e
        // alla riga 0 del secondo -> indici assoluti 1 e 2.
        let (_, batch_a) =
            coords_batch(vec![Some(1.0), Some(f64::NAN)], vec![Some(2.0), Some(4.0)]);
        let (_, batch_b) = coords_batch(vec![Some(f64::INFINITY)], vec![Some(2.0)]);
        let input = envelope_bytes(&schema, &[batch_a, batch_b]);
        let error = run(&arrow_schema(3, ArrowOperation::FromCoords), &input)
            .expect_err("coordinate non finite su due batch");
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 2);
        assert_eq!(report.counts["geometry.non_finite_coordinate"], 2);
        assert_eq!(
            report
                .examples
                .iter()
                .map(|example| example.source_index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        // Intero oltre 2^53: causa dedicata, colonna nominata, nessun valore.
        let (schema_big, big) = {
            let schema = Arc::new(Schema::new(vec![
                Field::new("x", DataType::Int64, true),
                Field::new("y", DataType::Float64, true),
            ]));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![Some(7_i64), Some((1_i64 << 53) + 1)])),
                    Arc::new(Float64Array::from(vec![Some(1.0), Some(2.0)])),
                ],
            )
            .expect("batch int");
            (schema, batch)
        };
        let renamed = TransformArrowSchema {
            x_column: Some("x".to_owned()),
            y_column: Some("y".to_owned()),
            ..arrow_schema(2, ArrowOperation::FromCoords)
        };
        let input = envelope_bytes(&schema_big, std::slice::from_ref(&big));
        let error = run(&renamed, &input).expect_err("intero oltre 2^53");
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["geometry.inexact_integer_coordinate"], 1);
        assert_eq!(report.examples[0].source_index, 1);
        assert_eq!(report.examples[0].column.as_deref(), Some("x"));
        assert!(report.validate_for_emission().is_ok());
    }

    #[test]
    fn from_coords_accepts_int64_coordinates() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, true),
            Field::new("y", DataType::Int64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(3_i64)])),
                Arc::new(Int64Array::from(vec![Some(4_i64)])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&schema, &[batch]);
        let output =
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input).expect("from_coords");
        let (out_schema, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::Point(Point::new(3.0, 4.0))
        );
    }

    #[test]
    fn from_coords_rejects_int64_beyond_f64_exact_range() {
        // Oltre 2^53 in valore assoluto la conversione i64 -> f64 non e'
        // esatta: la coordinata va rifiutata, mai spostata in silenzio.
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int64, true),
            Field::new("y", DataType::Int64, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some((1_i64 << 53) + 1)])),
                Arc::new(Int64Array::from(vec![Some(4_i64)])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&schema, &[batch]);
        let error = run(&arrow_schema(1, ArrowOperation::FromCoords), &input)
            .expect_err("coordinata intera oltre 2^53");
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["geometry.inexact_integer_coordinate"], 1);
        assert_eq!(report.examples[0].source_index, 0);

        // Il confine 2^53 e' esattamente rappresentabile: resta accettato.
        let boundary = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(1_i64 << 53)])),
                Arc::new(Int64Array::from(vec![Some(-(1_i64 << 53))])),
            ],
        )
        .unwrap();
        let input = envelope_bytes(&schema, &[boundary]);
        let output =
            run(&arrow_schema(1, ArrowOperation::FromCoords), &input).expect("from_coords");
        let (out_schema, out_batches) = decode_output(&output);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        assert_eq!(
            geometry_from_wkb(cells.value(0)).unwrap(),
            Geometry::Point(Point::new(2_f64.powi(53), -(2_f64.powi(53))))
        );
    }

    // --- Forma binary + lineage ---------------------------------------------

    fn pair_schema(operation: PairOperation, left_rows: u64, right_rows: u64) -> PairArrowSchema {
        PairArrowSchema {
            schema_version: PairArrowSchema::VERSION,
            operation,
            left_row_count: left_rows,
            right_row_count: right_rows,
            left_crs: Some(CRS.to_owned()),
            right_crs: Some(CRS.to_owned()),
            geometry_column: None,
            predicate: None,
            overlay_mode: None,
            max_pairs: None,
            max_comparisons: None,
            max_results: None,
            max_distance: None,
            max_output_rows: None,
            spatial_predicate: None,
            max_coordinate_pairs: None,
            tolerance: None,
        }
    }

    fn run_pair(
        schema: &PairArrowSchema,
        left: &[u8],
        right: &[u8],
    ) -> Result<Vec<u8>, ArrowTransportError> {
        let mut output = Vec::new();
        pair_arrow(left, right, &mut output, schema)?;
        Ok(output)
    }

    fn side_envelope(geometries: &[Option<&[u8]>]) -> Vec<u8> {
        let (schema, batch) = fixture_batch(geometries);
        envelope_bytes(&schema, std::slice::from_ref(&batch))
    }

    fn point_wkb(x: f64, y: f64) -> Vec<u8> {
        Geometry::Point(Point::new(x, y))
            .to_wkb(CoordDimensions::xy())
            .expect("point")
    }

    fn shifted_square_wkb(dx: f64, dy: f64, size: f64) -> Vec<u8> {
        Geometry::Polygon(polygon![
            (x: dx, y: dy), (x: dx + size, y: dy),
            (x: dx + size, y: dy + size), (x: dx, y: dy + size),
            (x: dx, y: dy),
        ])
        .to_wkb(CoordDimensions::xy())
        .expect("polygon")
    }

    #[test]
    fn sjoin_emits_deterministic_pairs_and_skips_nulls() {
        let left = side_envelope(&[
            Some(&shifted_square_wkb(0.0, 0.0, 2.0)),
            None,
            Some(&shifted_square_wkb(10.0, 10.0, 2.0)),
        ]);
        let right = side_envelope(&[
            None,
            Some(&shifted_square_wkb(1.0, 1.0, 2.0)),
            Some(&shifted_square_wkb(20.0, 20.0, 1.0)),
        ]);
        let schema = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::SJoin, 3, 3)
        };
        let output = run_pair(&schema, &left, &right).expect("sjoin");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches.len(), 1);
        let left_index = batches[0]
            .column(out_schema.index_of(LEFT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let right_index = batches[0]
            .column(out_schema.index_of(RIGHT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(left_index.values(), &[0]);
        assert_eq!(right_index.values(), &[1]);
        assert_eq!(out_schema.fields().len(), 2);

        // max_pairs obbligatorio e zero rifiutato.
        let missing = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            ..pair_schema(PairOperation::SJoin, 3, 3)
        };
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "max_pairs",
                ..
            })
        ));
        let zero = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(0),
            ..pair_schema(PairOperation::SJoin, 3, 3)
        };
        assert!(matches!(
            run_pair(&zero, &left, &right),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_pairs",
                ..
            })
        ));

        // row_count lato right non coerente.
        let mismatch = PairArrowSchema {
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::SJoin, 3, 2)
        };
        assert!(matches!(
            run_pair(&mismatch, &left, &right),
            Err(ArrowTransportError::PairRowCountMismatch { side: "right", .. })
        ));
    }

    #[test]
    fn distance_is_aligned_to_left_with_nulls_and_limit() {
        let left = side_envelope(&[
            Some(&point_wkb(0.0, 0.0)),
            None,
            Some(&point_wkb(10.0, 0.0)),
        ]);
        let right = side_envelope(&[Some(&point_wkb(3.0, 4.0)), Some(&point_wkb(10.0, 6.0))]);
        let schema = PairArrowSchema {
            max_comparisons: Some(100),
            ..pair_schema(PairOperation::Distance, 3, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("distance");
        let (out_schema, batches) = decode_output(&output);
        // colonne left invariate + distance in coda.
        assert_eq!(out_schema.fields().len(), 5);
        assert!(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).is_ok());
        let values = batches[0]
            .column(out_schema.index_of(DISTANCE_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(values.value(0), 5.0);
        assert!(values.is_null(1));
        assert_eq!(values.value(2), 6.0);
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);

        let limited = PairArrowSchema {
            max_comparisons: Some(1),
            ..pair_schema(PairOperation::Distance, 3, 2)
        };
        assert!(matches!(
            run_pair(&limited, &left, &right),
            Err(ArrowTransportError::Analysis(_))
        ));
    }

    #[test]
    fn nearest_emits_all_ties_with_distance() {
        let left = side_envelope(&[Some(&point_wkb(0.0, 0.0)), None]);
        let right = side_envelope(&[
            Some(&point_wkb(-1.0, 0.0)),
            None,
            Some(&point_wkb(1.0, 0.0)),
            Some(&point_wkb(5.0, 0.0)),
        ]);
        let schema = PairArrowSchema {
            max_comparisons: Some(100),
            max_results: Some(10),
            ..pair_schema(PairOperation::Nearest, 2, 4)
        };
        let output = run_pair(&schema, &left, &right).expect("nearest");
        let (out_schema, batches) = decode_output(&output);
        let left_index = batches[0]
            .column(out_schema.index_of(LEFT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let right_index = batches[0]
            .column(out_schema.index_of(RIGHT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let distances = batches[0]
            .column(out_schema.index_of(DISTANCE_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        // entrambi i pareggi a distanza 1, ordinati per indice right.
        assert_eq!(left_index.values(), &[0, 0]);
        assert_eq!(right_index.values(), &[0, 2]);
        assert_eq!(distances.values(), &[1.0, 1.0]);

        // max_results sotto i pareggi: errore dal kernel.
        let limited = PairArrowSchema {
            max_comparisons: Some(100),
            max_results: Some(1),
            ..pair_schema(PairOperation::Nearest, 2, 4)
        };
        assert!(matches!(
            run_pair(&limited, &left, &right),
            Err(ArrowTransportError::Analysis(_))
        ));

        // max_distance non finita: rifiutata dallo schema.
        let invalid = PairArrowSchema {
            max_comparisons: Some(100),
            max_results: Some(10),
            max_distance: Some(f64::NAN),
            ..pair_schema(PairOperation::Nearest, 2, 4)
        };
        assert!(matches!(
            run_pair(&invalid, &left, &right),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_distance",
                ..
            })
        ));
    }

    #[test]
    fn clip_marks_rows_outside_mask_as_null() {
        let inside = shifted_square_wkb(0.0, 0.0, 2.0);
        let outside = shifted_square_wkb(10.0, 10.0, 2.0);
        let left = side_envelope(&[Some(&inside), None, Some(&outside)]);
        let right = side_envelope(&[None, Some(&shifted_square_wkb(1.0, 1.0, 2.0))]);
        let schema = pair_schema(PairOperation::Clip, 3, 2);
        let output = run_pair(&schema, &left, &right).expect("clip");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches[0].num_rows(), 3);
        let cells = batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        // riga 0: intersezione 1x1; riga 1: null in input; riga 2: fuori maschera -> null.
        let clipped = geometry_from_wkb(cells.value(0)).unwrap();
        assert!((clipped.unsigned_area() - 1.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert!(cells.is_null(2));
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);
    }

    #[test]
    fn overlay_emits_pieces_with_nullable_lineage() {
        let left = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0)), None]);
        let right = side_envelope(&[Some(&shifted_square_wkb(1.0, 0.0, 2.0)), None]);

        for (mode, expected_pieces) in [
            (OverlayMode::Intersection, 1_usize),
            (OverlayMode::Union, 3),
            (OverlayMode::SymmetricDifference, 2),
            (OverlayMode::Identity, 2),
        ] {
            let schema = PairArrowSchema {
                overlay_mode: Some(mode),
                max_pairs: Some(10),
                ..pair_schema(PairOperation::Overlay, 2, 2)
            };
            let output = run_pair(&schema, &left, &right).expect("overlay");
            let (out_schema, batches) = decode_output(&output);
            assert_eq!(batches[0].num_rows(), expected_pieces, "mode {mode:?}");
            let left_index = batches[0]
                .column(out_schema.index_of(LEFT_INDEX_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            let right_index = batches[0]
                .column(out_schema.index_of(RIGHT_INDEX_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap();
            match mode {
                OverlayMode::Intersection => {
                    assert!(!left_index.is_null(0) && !right_index.is_null(0));
                }
                OverlayMode::Union | OverlayMode::SymmetricDifference => {
                    // pezzi con un solo lato: l'altro indice e' null.
                    let left_nulls = (0..expected_pieces)
                        .filter(|&i| left_index.is_null(i))
                        .count();
                    let right_nulls = (0..expected_pieces)
                        .filter(|&i| right_index.is_null(i))
                        .count();
                    assert_eq!(left_nulls, 1, "mode {mode:?}");
                    assert_eq!(right_nulls, 1, "mode {mode:?}");
                }
                OverlayMode::Identity => {
                    let right_nulls = (0..expected_pieces)
                        .filter(|&i| right_index.is_null(i))
                        .count();
                    assert_eq!(right_nulls, 1);
                }
            }
            let cells = batches[0]
                .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let total: f64 = (0..expected_pieces)
                .map(|i| geometry_from_wkb(cells.value(i)).unwrap().unsigned_area())
                .sum();
            // Bracci separati per costruzione: ogni OverlayMode ha una
            // semantica diversa; 4.0 coincide per SymmetricDifference e
            // Identity solo su questa fixture, non per lo stesso caso.
            #[allow(clippy::match_same_arms)]
            let expected_area = match mode {
                OverlayMode::Intersection => 2.0,
                OverlayMode::Union => 6.0,
                OverlayMode::SymmetricDifference => 4.0,
                OverlayMode::Identity => 4.0,
            };
            assert!(
                (total - expected_area).abs() < 1e-9,
                "mode {mode:?}: {total}"
            );
        }

        // overlay_mode obbligatorio.
        let missing = PairArrowSchema {
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Overlay, 2, 2)
        };
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "overlay_mode",
                ..
            })
        ));

        // input non poligonale: rifiutato dal kernel.
        let bad_left = side_envelope(&[Some(&point_wkb(0.0, 0.0)), None]);
        let schema = PairArrowSchema {
            overlay_mode: Some(OverlayMode::Intersection),
            max_pairs: Some(10),
            ..pair_schema(PairOperation::Overlay, 2, 2)
        };
        assert!(matches!(
            run_pair(&schema, &bad_left, &right),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    #[test]
    fn pair_requires_both_crs_declarations() {
        let left = side_envelope(&[None]);
        let right = side_envelope(&[None]);
        let schema = PairArrowSchema {
            right_crs: None,
            predicate: Some(JoinPredicate::Intersects),
            max_pairs: Some(1),
            ..pair_schema(PairOperation::SJoin, 1, 1)
        };
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::CrsRequired)
        ));
    }

    #[test]
    fn within_outputs_strict_boolean_aligned_to_left() {
        let left = side_envelope(&[
            Some(&point_wkb(0.5, 0.5)), // dentro
            Some(&point_wkb(0.0, 1.0)), // sul bordo: strict-within -> false
            Some(&point_wkb(5.0, 5.0)), // fuori
            None,
        ]);
        let right = side_envelope(&[Some(&shifted_square_wkb(0.0, 0.0, 2.0)), None]);
        let schema = PairArrowSchema {
            max_pairs: Some(100),
            ..pair_schema(PairOperation::Within, 4, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("within");
        let (out_schema, batches) = decode_output(&output);
        // colonne left invariate + `within` Boolean in coda.
        assert_eq!(out_schema.fields().len(), 5);
        let flags = batches[0]
            .column(out_schema.index_of(WITHIN_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
            .unwrap();
        assert!(flags.value(0));
        assert!(!flags.value(1));
        assert!(!flags.value(2));
        assert!(flags.is_null(3));

        let missing = pair_schema(PairOperation::Within, 4, 2);
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "max_pairs",
                ..
            })
        ));
    }

    #[test]
    fn count_points_in_polygons_counts_strict_within_only() {
        let left = side_envelope(&[
            Some(&shifted_square_wkb(0.0, 0.0, 2.0)),
            Some(&shifted_square_wkb(10.0, 10.0, 2.0)),
            None,
        ]);
        let right = side_envelope(&[
            Some(&point_wkb(0.5, 0.5)),
            Some(&point_wkb(1.5, 1.5)),
            Some(&point_wkb(0.0, 1.0)), // bordo: non contato
            Some(&point_wkb(10.5, 10.5)),
            Some(&point_wkb(50.0, 50.0)),
            None,
        ]);
        let schema = PairArrowSchema {
            max_pairs: Some(100),
            ..pair_schema(PairOperation::CountPointsInPolygons, 3, 6)
        };
        let output = run_pair(&schema, &left, &right).expect("count");
        let (out_schema, batches) = decode_output(&output);
        let counts = batches[0]
            .column(out_schema.index_of(COUNT_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(counts.value(0), 2);
        assert_eq!(counts.value(1), 1);
        assert!(counts.is_null(2));
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);
    }

    #[test]
    fn pairwise_booleans_align_rows_and_map_empty_to_null() {
        let overlapping_a = shifted_square_wkb(0.0, 0.0, 2.0);
        let overlapping_b = shifted_square_wkb(1.0, 0.0, 2.0);
        let far = shifted_square_wkb(10.0, 10.0, 1.0);
        let left = side_envelope(&[
            Some(&overlapping_a),
            Some(&overlapping_a),
            Some(&overlapping_a),
            None,
        ]);
        let right = side_envelope(&[
            Some(&overlapping_b),
            Some(&far),
            Some(&overlapping_a),
            Some(&overlapping_b),
        ]);

        let run_op = |operation: PairOperation| {
            let schema = pair_schema(operation, 4, 4);
            let output =
                run_pair(&schema, &left, &right).unwrap_or_else(|_| panic!("{}", operation.name()));
            let (out_schema, batches) = decode_output(&output);
            let cells = batches[0]
                .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
                .as_any()
                .downcast_ref::<BinaryArray>()
                .unwrap();
            let areas: Vec<Option<f64>> = (0..4)
                .map(|row| {
                    (!cells.is_null(row))
                        .then(|| geometry_from_wkb(cells.value(row)).unwrap().unsigned_area())
                })
                .collect();
            areas
        };

        // intersection: overlap 1x2=2, disgiunti -> null, uguali -> 4, left null -> null.
        let areas = run_op(PairOperation::Intersection);
        assert!((areas[0].unwrap() - 2.0).abs() < 1e-12);
        assert!(areas[1].is_none());
        assert!((areas[2].unwrap() - 4.0).abs() < 1e-12);
        assert!(areas[3].is_none());

        // union: 4+4-2=6; disgiunti 4+1=5; uguali 4; null -> null.
        let areas = run_op(PairOperation::Union);
        assert!((areas[0].unwrap() - 6.0).abs() < 1e-12);
        assert!((areas[1].unwrap() - 5.0).abs() < 1e-12);
        assert!((areas[2].unwrap() - 4.0).abs() < 1e-12);
        assert!(areas[3].is_none());

        // difference: A\B=2; A\far=4; A\A -> null (EMPTY).
        let areas = run_op(PairOperation::Difference);
        assert!((areas[0].unwrap() - 2.0).abs() < 1e-12);
        assert!((areas[1].unwrap() - 4.0).abs() < 1e-12);
        assert!(areas[2].is_none());
        assert!(areas[3].is_none());

        // symmetric_difference: 4; 5; uguale -> null.
        let areas = run_op(PairOperation::SymmetricDifference);
        assert!((areas[0].unwrap() - 4.0).abs() < 1e-12);
        assert!((areas[1].unwrap() - 5.0).abs() < 1e-12);
        assert!(areas[2].is_none());

        // row_count non allineati: fail-closed.
        let mismatched_right = side_envelope(&[Some(&overlapping_b)]);
        let mut schema = pair_schema(PairOperation::Intersection, 4, 1);
        schema.max_output_rows = Some(0);
        assert!(matches!(
            run_pair(&schema, &left, &mismatched_right),
            Err(ArrowTransportError::SideLengthMismatch { left: 4, right: 1 })
        ));

        // input non poligonale: rifiutato dal kernel.
        let bad_left = side_envelope(&[Some(&point_wkb(0.0, 0.0))]);
        let bad_right = side_envelope(&[Some(&point_wkb(1.0, 1.0))]);
        let schema = pair_schema(PairOperation::Union, 1, 1);
        assert!(matches!(
            run_pair(&schema, &bad_left, &bad_right),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    #[test]
    fn clean_topology_applies_first_row_wins_and_preserves_positions() {
        let square = shifted_square_wkb(0.0, 0.0, 2.0);
        let duplicate = shifted_square_wkb(0.0, 0.0, 2.0);
        let separate = shifted_square_wkb(10.0, 10.0, 2.0);
        let (schema, batch) =
            fixture_batch(&[Some(&square), Some(&duplicate), Some(&separate), None]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let clean_schema = TransformArrowSchema {
            snap_tolerance: Some(0.0),
            ..arrow_schema(4, ArrowOperation::CleanTopology)
        };
        let output = run(&clean_schema, &input).expect("clean_topology");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_batches[0].num_rows(), 4);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        // riga 0 conservata; la duplicata e' assorbita (first-row-wins) -> null;
        // la separata conservata; null in input -> null.
        assert!((geometry_from_wkb(cells.value(0)).unwrap().unsigned_area() - 4.0).abs() < 1e-12);
        assert!(cells.is_null(1));
        assert!((geometry_from_wkb(cells.value(2)).unwrap().unsigned_area() - 4.0).abs() < 1e-12);
        assert!(cells.is_null(3));
        let ids = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2, 3]);

        // snap_tolerance obbligatoria e non negativa.
        let missing = arrow_schema(4, ArrowOperation::CleanTopology);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "snap_tolerance",
                ..
            })
        ));
        let negative = TransformArrowSchema {
            snap_tolerance: Some(-0.5),
            ..arrow_schema(4, ArrowOperation::CleanTopology)
        };
        assert!(matches!(
            run(&negative, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "snap_tolerance",
                ..
            })
        ));

        // input non poligonale: rifiutato dal kernel.
        let line = line_wkb();
        let (schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&schema, std::slice::from_ref(&batch));
        let single = TransformArrowSchema {
            snap_tolerance: Some(0.0),
            ..arrow_schema(1, ArrowOperation::CleanTopology)
        };
        assert!(matches!(
            run(&single, &input),
            Err(ArrowTransportError::Topology(_))
        ));
    }

    // --- Estensioni di catalogo ----------------------------------------------

    fn single_geometry_output(output: &[u8]) -> Geometry<f64> {
        let (out_schema, out_batches) = decode_output(output);
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        geometry_from_wkb(cells.value(0)).expect("decode")
    }

    fn run_single(
        schema: &TransformArrowSchema,
        wkb: &[u8],
    ) -> Result<Vec<u8>, ArrowTransportError> {
        let (fixture_schema, batch) = fixture_batch(&[Some(wkb)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        run(schema, &input)
    }

    #[test]
    fn affine_family_transforms_geometry_and_validates_params() {
        let square = square_wkb(2.0);

        let schema = TransformArrowSchema {
            x_offset: Some(10.0),
            y_offset: Some(-5.0),
            ..arrow_schema(1, ArrowOperation::Translate)
        };
        let output = run_single(&schema, &square).expect("translate");
        let Geometry::Polygon(translated) = single_geometry_output(&output) else {
            panic!("atteso Polygon")
        };
        assert_eq!(translated.exterior().0[0], geo::Coord { x: 10.0, y: -5.0 });

        let schema = TransformArrowSchema {
            x_factor: Some(2.0),
            y_factor: Some(2.0),
            ..arrow_schema(1, ArrowOperation::Scale)
        };
        let output = run_single(&schema, &square).expect("scale");
        assert!((single_geometry_output(&output).unsigned_area() - 16.0).abs() < 1e-12);

        let schema = TransformArrowSchema {
            degrees: Some(90.0),
            ..arrow_schema(1, ArrowOperation::Rotate)
        };
        let output = run_single(&schema, &square).expect("rotate");
        assert!((single_geometry_output(&output).unsigned_area() - 4.0).abs() < 1e-12);

        let schema = TransformArrowSchema {
            coefficients: Some(vec![1.0, 0.0, 5.0, 0.0, 1.0, 5.0]),
            ..arrow_schema(1, ArrowOperation::AffineTransform)
        };
        let output = run_single(&schema, &square).expect("affine");
        let Geometry::Polygon(shifted) = single_geometry_output(&output) else {
            panic!("atteso Polygon")
        };
        assert_eq!(shifted.exterior().0[0], geo::Coord { x: 5.0, y: 5.0 });

        let scattered = Geometry::MultiPoint(geo::MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(4.0, 0.5),
            Point::new(3.5, 3.0),
            Point::new(1.0, 4.0),
            Point::new(-0.5, 2.0),
            Point::new(2.0, 2.0),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("scattered");
        let schema = TransformArrowSchema {
            concavity: Some(2.0),
            ..arrow_schema(1, ArrowOperation::ConcaveHull)
        };
        let output = run_single(&schema, &scattered).expect("concave_hull");
        assert!(matches!(
            single_geometry_output(&output),
            Geometry::Polygon(_)
        ));

        // parametri invalidi e non applicabili.
        let missing = arrow_schema(1, ArrowOperation::Translate);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "x_offset",
                ..
            })
        ));
        let bad_coefficients = TransformArrowSchema {
            coefficients: Some(vec![1.0, 0.0, 0.0, 0.0, 1.0]),
            ..arrow_schema(1, ArrowOperation::AffineTransform)
        };
        assert!(matches!(
            run(&bad_coefficients, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "coefficients",
                ..
            })
        ));
        let zero_concavity = TransformArrowSchema {
            concavity: Some(0.0),
            ..arrow_schema(1, ArrowOperation::ConcaveHull)
        };
        assert!(matches!(
            run(&zero_concavity, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "concavity",
                ..
            })
        ));
        let unexpected = TransformArrowSchema {
            x_offset: Some(1.0),
            y_offset: Some(1.0),
            degrees: Some(90.0),
            ..arrow_schema(1, ArrowOperation::Translate)
        };
        assert!(matches!(
            run(&unexpected, &input),
            Err(ArrowTransportError::UnexpectedParameter {
                name: "degrees",
                ..
            })
        ));
    }

    #[test]
    fn densify_and_snap_to_grid_transform_cells() {
        let line = line_wkb();
        let schema = TransformArrowSchema {
            max_segment_length: Some(1.0),
            ..arrow_schema(1, ArrowOperation::Densify)
        };
        let output = run_single(&schema, &line).expect("densify");
        let densified = single_geometry_output(&output);
        assert!(densified.coords_count() > 3);

        let schema = TransformArrowSchema {
            grid_size: Some(0.5),
            ..arrow_schema(1, ArrowOperation::SnapToGrid)
        };
        let output = run_single(&schema, &line).expect("snap");
        let snapped = single_geometry_output(&output);
        assert!(snapped
            .coords_iter()
            .all(|c| (c.x * 2.0).fract() == 0.0 && (c.y * 2.0).fract() == 0.0));

        let (fixture_schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let invalid = TransformArrowSchema {
            max_segment_length: Some(0.0),
            ..arrow_schema(1, ArrowOperation::Densify)
        };
        assert!(matches!(
            run(&invalid, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "max_segment_length",
                ..
            })
        ));
        let invalid_grid = TransformArrowSchema {
            grid_size: Some(-1.0),
            ..arrow_schema(1, ArrowOperation::SnapToGrid)
        };
        assert!(matches!(
            run(&invalid_grid, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "grid_size",
                ..
            })
        ));
    }

    #[test]
    fn line_reference_ops_require_lines_and_valid_ratios() {
        let mut line = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&10.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());

        let schema = TransformArrowSchema {
            start_ratio: Some(0.25),
            end_ratio: Some(0.75),
            ..arrow_schema(1, ArrowOperation::LineSubstring)
        };
        let output = run_single(&schema, &line).expect("substring");
        let Geometry::LineString(piece) = single_geometry_output(&output) else {
            panic!("atteso LineString")
        };
        assert_eq!(piece.0.first().unwrap().x, 2.5);
        assert_eq!(piece.0.last().unwrap().x, 7.5);

        let schema = TransformArrowSchema {
            ratio: Some(0.5),
            ..arrow_schema(1, ArrowOperation::LineInterpolatePoint)
        };
        let output = run_single(&schema, &line).expect("interpolate");
        assert_eq!(
            single_geometry_output(&output),
            Geometry::Point(Point::new(5.0, 0.0))
        );

        let square = square_wkb(1.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let schema = TransformArrowSchema {
            ratio: Some(0.5),
            ..arrow_schema(1, ArrowOperation::LineInterpolatePoint)
        };
        let error = run(&schema, &input).expect_err("tipo geometria errato");
        assert!(matches!(
            error.source_error(),
            ArrowTransportError::WrongGeometryType { .. }
        ));
        // Difetto row-scoped: la riga e' rifiutata con diagnostica completa.
        let report = error.row_diagnostics().expect("diagnostica row-scoped");
        assert_eq!(report.observed_total, 1);
        assert_eq!(report.counts["geometry.wrong_type"], 1);

        let (fixture_schema, batch) = fixture_batch(&[Some(&line)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let invalid = TransformArrowSchema {
            ratio: Some(1.5),
            ..arrow_schema(1, ArrowOperation::LineInterpolatePoint)
        };
        assert!(matches!(
            run(&invalid, &input),
            Err(ArrowTransportError::InvalidParameter { name: "ratio", .. })
        ));
        let inverted = TransformArrowSchema {
            start_ratio: Some(0.8),
            end_ratio: Some(0.2),
            ..arrow_schema(1, ArrowOperation::LineSubstring)
        };
        assert!(matches!(
            run(&inverted, &input),
            Err(ArrowTransportError::InvalidParameter {
                name: "start_ratio/end_ratio",
                ..
            })
        ));
    }

    #[test]
    fn geodesic_unary_ops_measure_lines_and_areas() {
        let mut line = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&1.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        let output = run_single(&arrow_schema(1, ArrowOperation::GeodesicLineLength), &line)
            .expect("geodesic_line_length");
        let (out_schema, out_batches) = decode_output(&output);
        let values = out_batches[0]
            .column(out_schema.index_of("geodesic_line_length").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((values.value(0) - 111_319.5).abs() / 111_319.5 < 1e-3);

        let square = square_wkb(1.0);
        let output = run_single(&arrow_schema(1, ArrowOperation::GeodesicArea), &square)
            .expect("geodesic_area");
        let (out_schema, out_batches) = decode_output(&output);
        let values = out_batches[0]
            .column(out_schema.index_of("geodesic_area").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((values.value(0) - 1.2309e10).abs() / 1.2309e10 < 1e-3);

        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::GeodesicLineLength), &input)
                .expect_err("tipo geometria errato")
                .source_error(),
            ArrowTransportError::WrongGeometryType { .. }
        ));
    }

    #[test]
    fn geometry_diagnostics_accepts_invalid_input_and_reports_structure() {
        let mut bowtie = vec![1_u8];
        bowtie.extend_from_slice(&3_u32.to_le_bytes());
        bowtie.extend_from_slice(&1_u32.to_le_bytes());
        bowtie.extend_from_slice(&5_u32.to_le_bytes());
        for (x, y) in [
            (0.0_f64, 0.0_f64),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0),
            (0.0, 0.0),
        ] {
            bowtie.extend_from_slice(&x.to_le_bytes());
            bowtie.extend_from_slice(&y.to_le_bytes());
        }
        let square = square_wkb(2.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&bowtie), Some(&square), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(
            &arrow_schema(3, ArrowOperation::GeometryDiagnostics),
            &input,
        )
        .expect("diagnostics");
        let (out_schema, out_batches) = decode_output(&output);
        let batch = &out_batches[0];
        let column = |name: &str| batch.column(out_schema.index_of(name).unwrap()).clone();

        let is_valid = column("is_valid");
        let is_valid = is_valid
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
            .unwrap();
        assert!(!is_valid.value(0));
        assert!(is_valid.value(1));
        assert!(is_valid.is_null(2));

        let reasons = column("validity_reason");
        let reasons = reasons.as_any().downcast_ref::<StringArray>().unwrap();
        assert!(!reasons.is_null(0));
        assert!(reasons.is_null(1));

        let types = column("geometry_type");
        let types = types.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(types.value(0), "Polygon");
        let counts = column("coordinate_count");
        let counts = counts.as_any().downcast_ref::<UInt64Array>().unwrap();
        assert_eq!(counts.value(1), 5);
        let bounds_maxx = column("bounds_maxx");
        let bounds_maxx = bounds_maxx.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!(bounds_maxx.value(1), 2.0);
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 1, 2]);
    }

    #[test]
    fn delaunay_expands_triangles_with_lineage_and_limit() {
        let multi = Geometry::MultiPoint(geo::MultiPoint::new(vec![
            Point::new(0.0, 0.0),
            Point::new(3.0, 0.5),
            Point::new(1.0, 2.5),
        ]))
        .to_wkb(CoordDimensions::xy())
        .expect("multi");
        let square = square_wkb(1.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&multi), None, Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let schema = TransformArrowSchema {
            max_output_rows: Some(64),
            ..arrow_schema(3, ArrowOperation::Delaunay)
        };
        let output = run(&schema, &input).expect("delaunay");
        let (out_schema, out_batches) = decode_output(&output);
        let parents = out_batches[0]
            .column(out_schema.index_of(PARENT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        let rows = out_batches[0].num_rows();
        assert!(rows >= 2);
        assert!(parents.values().iter().all(|&p| p == 0 || p == 2));
        assert!(parents.values().contains(&0));
        assert!(parents.values().contains(&2));
        let cells = out_batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let first = geometry_from_wkb(cells.value(0)).unwrap();
        assert!(matches!(first, Geometry::Polygon(_)));
        assert_eq!(first.coords_count(), 4);

        let missing = arrow_schema(3, ArrowOperation::Delaunay);
        assert!(matches!(
            run(&missing, &input),
            Err(ArrowTransportError::MissingParameter {
                name: "max_output_rows",
                ..
            })
        ));
    }

    #[test]
    fn line_merge_merges_maximal_paths_only() {
        let mut lines_wkb = vec![1_u8, 7, 0, 0, 0, 3, 0, 0, 0];
        let segments: [[(f64, f64); 2]; 3] = [
            [(0.0, 0.0), (1.0, 0.0)],
            [(1.0, 0.0), (2.0, 0.0)],
            [(5.0, 5.0), (6.0, 6.0)],
        ];
        for segment in segments {
            lines_wkb.push(1);
            lines_wkb.extend_from_slice(&2_u32.to_le_bytes());
            lines_wkb.extend_from_slice(&2_u32.to_le_bytes());
            for (x, y) in segment {
                lines_wkb.extend_from_slice(&x.to_le_bytes());
                lines_wkb.extend_from_slice(&y.to_le_bytes());
            }
        }
        let (fixture_schema, batch) = fixture_batch(&[Some(&lines_wkb), None]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(2, ArrowOperation::LineMerge), &input).expect("line_merge");
        let (out_schema, out_batches) = decode_output(&output);
        assert_eq!(out_schema.fields().len(), 1);
        assert_eq!(out_batches[0].num_rows(), 2);
        let cells = out_batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let merged: Vec<usize> = (0..2)
            .map(|row| geometry_from_wkb(cells.value(row)).unwrap().coords_count())
            .collect();
        assert!(merged.contains(&3));
        assert!(merged.contains(&2));
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn polygonize_classifies_faces_and_residuals() {
        // quadrato chiuso + dangle: attesi un poligono e un dangle.
        let mut collection = vec![1_u8, 7, 0, 0, 0, 2, 0, 0, 0];
        let ring_groups: [&[(f64, f64)]; 2] = [
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)].as_slice(),
            [(1.0, 1.0), (3.0, 3.0)].as_slice(),
        ];
        for ring in ring_groups {
            collection.push(1);
            collection.extend_from_slice(&2_u32.to_le_bytes());
            collection.extend_from_slice(
                &u32::try_from(ring.len())
                    .expect("fixture: anello sotto u32::MAX")
                    .to_le_bytes(),
            );
            for (x, y) in ring {
                collection.extend_from_slice(&x.to_le_bytes());
                collection.extend_from_slice(&y.to_le_bytes());
            }
        }
        let (fixture_schema, batch) = fixture_batch(&[Some(&collection)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        let output = run(&arrow_schema(1, ArrowOperation::Polygonize), &input).expect("polygonize");
        let (out_schema, out_batches) = decode_output(&output);
        let classes = out_batches[0]
            .column(out_schema.index_of(CLASS_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let classes: Vec<&str> = (0..out_batches[0].num_rows())
            .map(|row| classes.value(row))
            .collect();
        assert!(classes.contains(&"polygon"));
        assert!(classes.contains(&"dangle"));
        assert_eq!(classes.len(), 2);
    }

    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn polygonize_without_geos_fails_closed() {
        let square = square_wkb(1.0);
        let (fixture_schema, batch) = fixture_batch(&[Some(&square)]);
        let input = envelope_bytes(&fixture_schema, std::slice::from_ref(&batch));
        assert!(matches!(
            run(&arrow_schema(1, ArrowOperation::Polygonize), &input),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "geos-backend",
                ..
            })
        ));
    }

    #[test]
    fn predicate_pair_op_aligns_rows_and_names_column() {
        let square = shifted_square_wkb(0.0, 0.0, 2.0);
        let inside = point_wkb(1.0, 1.0);
        let boundary = point_wkb(0.0, 1.0);
        let left = side_envelope(&[Some(&square), Some(&square), None]);
        let right = side_envelope(&[Some(&inside), Some(&boundary), Some(&inside)]);
        let schema = PairArrowSchema {
            spatial_predicate: Some(SpatialPredicate::Covers),
            ..pair_schema(PairOperation::Predicate, 3, 3)
        };
        let output = run_pair(&schema, &left, &right).expect("predicate");
        let (out_schema, batches) = decode_output(&output);
        let flags = batches[0]
            .column(out_schema.index_of("predicate_covers").unwrap())
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
            .unwrap();
        assert!(flags.value(0));
        assert!(flags.value(1));
        assert!(flags.is_null(2));

        let missing = pair_schema(PairOperation::Predicate, 3, 3);
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "spatial_predicate",
                ..
            })
        ));

        let short_right = side_envelope(&[Some(&inside)]);
        let schema = PairArrowSchema {
            spatial_predicate: Some(SpatialPredicate::Intersects),
            ..pair_schema(PairOperation::Predicate, 3, 1)
        };
        assert!(matches!(
            run_pair(&schema, &left, &short_right),
            Err(ArrowTransportError::SideLengthMismatch { .. })
        ));
    }

    #[test]
    fn hausdorff_and_frechet_are_pairwise_with_limits() {
        let line_a = line_wkb();
        let line_b = {
            let mut wkb = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
            wkb.extend_from_slice(&0.0_f64.to_le_bytes());
            wkb.extend_from_slice(&1.0_f64.to_le_bytes());
            wkb.extend_from_slice(&3.0_f64.to_le_bytes());
            wkb.extend_from_slice(&5.0_f64.to_le_bytes());
            wkb
        };
        let left = side_envelope(&[Some(&line_a), None]);
        let right = side_envelope(&[Some(&line_b), Some(&line_b)]);

        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1000),
            ..pair_schema(PairOperation::HausdorffDistance, 2, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("hausdorff");
        let (out_schema, batches) = decode_output(&output);
        let values = batches[0]
            .column(out_schema.index_of("hausdorff_distance").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(values.value(0) > 0.0);
        assert!(values.is_null(1));

        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1000),
            ..pair_schema(PairOperation::FrechetDistance, 2, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("frechet");
        let (out_schema, batches) = decode_output(&output);
        let values = batches[0]
            .column(out_schema.index_of("frechet_distance").unwrap())
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(values.value(0) > 0.0);

        // tipo sbagliato per frechet e limite di lavoro.
        let square = shifted_square_wkb(0.0, 0.0, 1.0);
        let bad = side_envelope(&[Some(&square), None]);
        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1000),
            ..pair_schema(PairOperation::FrechetDistance, 2, 2)
        };
        assert!(matches!(
            run_pair(&schema, &bad, &right),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
        let schema = PairArrowSchema {
            max_coordinate_pairs: Some(1),
            ..pair_schema(PairOperation::HausdorffDistance, 2, 2)
        };
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::Extended(_))
        ));
        let missing = pair_schema(PairOperation::HausdorffDistance, 2, 2);
        assert!(matches!(
            run_pair(&missing, &left, &right),
            Err(ArrowTransportError::MissingParameter {
                name: "max_coordinate_pairs",
                ..
            })
        ));
    }

    #[test]
    fn geodesic_pair_ops_measure_between_points() {
        let rome = point_wkb(12.0, 41.0);
        let milan = point_wkb(9.0, 45.0);
        let left = side_envelope(&[Some(&rome), None]);
        let right = side_envelope(&[Some(&milan), Some(&milan)]);
        for (operation, column, expected, tolerance) in [
            (
                PairOperation::HaversineDistance,
                "haversine_distance",
                507_205.0,
                0.01,
            ),
            (
                PairOperation::GeodesicDistance,
                "geodesic_distance",
                507_161.0,
                0.01,
            ),
            (PairOperation::Bearing, "bearing", 332.2, 0.05),
        ] {
            let schema = pair_schema(operation, 2, 2);
            let output =
                run_pair(&schema, &left, &right).unwrap_or_else(|_| panic!("{}", operation.name()));
            let (out_schema, batches) = decode_output(&output);
            let values = batches[0]
                .column(out_schema.index_of(column).unwrap())
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let actual = values.value(0);
            assert!(
                (actual - expected).abs() / expected < tolerance,
                "{}: {actual} vs {expected}",
                operation.name()
            );
            assert!(values.is_null(1));
        }

        // tipo sbagliato: non Point.
        let square = shifted_square_wkb(0.0, 0.0, 1.0);
        let bad = side_envelope(&[Some(&square), None]);
        let schema = pair_schema(PairOperation::HaversineDistance, 2, 2);
        assert!(matches!(
            run_pair(&schema, &bad, &right),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
    }

    #[cfg(feature = "geos-backend")]
    #[test]
    fn split_produces_pieces_with_lineage_and_conserves_measures() {
        let mut line = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        line.extend_from_slice(&10.0_f64.to_le_bytes());
        line.extend_from_slice(&0.0_f64.to_le_bytes());
        let cutter = point_wkb(5.0, 0.0);
        let left = side_envelope(&[Some(&line), None]);
        let right = side_envelope(&[Some(&cutter), Some(&cutter)]);
        let schema = PairArrowSchema {
            max_output_rows: Some(16),
            ..pair_schema(PairOperation::Split, 2, 2)
        };
        let output = run_pair(&schema, &left, &right).expect("split lineare");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches[0].num_rows(), 2);
        let parents = batches[0]
            .column(out_schema.index_of(PARENT_INDEX_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(parents.values(), &[0, 0]);
        let ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ids.values(), &[0, 0]);
        let cells = batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let total: f64 = (0..2)
            .map(|row| match geometry_from_wkb(cells.value(row)).unwrap() {
                Geometry::LineString(piece) => geo::algorithm::line_measures::Length::length(
                    &geo::algorithm::line_measures::Euclidean,
                    &piece,
                ),
                other => panic!("atteso LineString: {other:?}"),
            })
            .sum();
        assert!((total - 10.0).abs() < 1e-9);

        // split poligonale: quadrato tagliato da una retta verticale.
        let square = shifted_square_wkb(0.0, 0.0, 2.0);
        let mut blade = vec![1_u8, 2, 0, 0, 0, 2, 0, 0, 0];
        blade.extend_from_slice(&1.0_f64.to_le_bytes());
        blade.extend_from_slice(&(-1.0_f64).to_le_bytes());
        blade.extend_from_slice(&1.0_f64.to_le_bytes());
        blade.extend_from_slice(&3.0_f64.to_le_bytes());
        let left = side_envelope(&[Some(&square)]);
        let right = side_envelope(&[Some(&blade)]);
        let schema = PairArrowSchema {
            max_output_rows: Some(16),
            ..pair_schema(PairOperation::Split, 1, 1)
        };
        let output = run_pair(&schema, &left, &right).expect("split poligonale");
        let (out_schema, batches) = decode_output(&output);
        assert_eq!(batches[0].num_rows(), 2);
        let cells = batches[0]
            .column(out_schema.index_of(DEFAULT_GEOMETRY_COLUMN).unwrap())
            .as_any()
            .downcast_ref::<BinaryArray>()
            .unwrap();
        let total: f64 = (0..2)
            .map(|row| geometry_from_wkb(cells.value(row)).unwrap().unsigned_area())
            .sum();
        assert!((total - 4.0).abs() < 1e-9);

        // tipo sorgente non supportato.
        let point = point_wkb(0.0, 0.0);
        let bad = side_envelope(&[Some(&point)]);
        assert!(matches!(
            run_pair(&schema, &bad, &right),
            Err(ArrowTransportError::WrongGeometryType { .. })
        ));
    }

    #[cfg(not(feature = "geos-backend"))]
    #[test]
    fn split_without_geos_fails_closed() {
        let left = side_envelope(&[None]);
        let right = side_envelope(&[None]);
        let schema = pair_schema(PairOperation::Split, 1, 1);
        assert!(matches!(
            run_pair(&schema, &left, &right),
            Err(ArrowTransportError::BackendUnavailable {
                feature: "geos-backend",
                ..
            })
        ));
    }
}
