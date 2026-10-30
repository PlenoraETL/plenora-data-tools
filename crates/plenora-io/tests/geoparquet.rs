//! `GeoParquet` 1.1: lettura di file esterni e costruiti a mano, scrittura,
//! andata e ritorno, CRS.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// Le fixture si costruiscono al volo e passano per valore agli helper.
#![allow(clippy::needless_pass_by_value)]

mod comune;

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use plenora_core::arrow::array::{
    Array, ArrayRef, BinaryArray, Int64Array, LargeBinaryArray, RecordBatch,
};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{
    geometry_output_field, geometry_output_field_with_dimensions, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION, GEO_METADATA_KEY, PLENORA_CONTRACT_VERSION_KEY,
    PLENORA_GEOMETRY_AXIS_ORDER_KEY, PLENORA_GEOMETRY_CRS_ID_KEY,
    PLENORA_GEOMETRY_CRS_RESOLUTION_KEY, PLENORA_GEOMETRY_ENCODING_KEY,
    PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, PLENORA_GEOMETRY_TYPES_KEY,
};
use plenora_core::contract::arrow_schema::contract_from_arrow_schema;
use plenora_core::contract::{ContractCrs, DataContract, GeometryDimensions, GeometryType};
use plenora_core::crs::resolve_crs;
use plenora_core::{ErrorCategory, PlenoraError};
use plenora_io::crs_projjson::projjson_di;
use plenora_io::{leggi_tabella, scrivi_tabella, OpzioniScrittura};
use serde_json::{json, Value};

use comune::cartella;

fn fixture(nome: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dati")
        .join(nome)
}

fn punto(x: f64, y: f64) -> Vec<u8> {
    let mut v = vec![1_u8];
    v.extend_from_slice(&1_u32.to_le_bytes());
    v.extend_from_slice(&x.to_le_bytes());
    v.extend_from_slice(&y.to_le_bytes());
    v
}

fn linea(coordinate: &[(f64, f64)]) -> Vec<u8> {
    let mut v = vec![1_u8];
    v.extend_from_slice(&2_u32.to_le_bytes());
    v.extend_from_slice(&u32::try_from(coordinate.len()).unwrap().to_le_bytes());
    for (x, y) in coordinate {
        v.extend_from_slice(&x.to_le_bytes());
        v.extend_from_slice(&y.to_le_bytes());
    }
    v
}

fn contratto(tabella: &RecordBatch) -> DataContract {
    contract_from_arrow_schema(tabella.schema(), resolve_crs).expect("contratto")
}

fn identificativo(crs: &ContractCrs) -> String {
    plenora_io::crs_projjson::identificativo_di(crs.as_resolved().expect("risolto")).unwrap()
}

/// Il metadato `geo` di un file Parquet, come JSON.
fn geo_del_file(percorso: &Path) -> Value {
    let costruttore =
        ParquetRecordBatchReaderBuilder::try_new(File::open(percorso).unwrap()).unwrap();
    let voce = costruttore
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .unwrap()
        .iter()
        .find(|voce| voce.key == "geo")
        .expect("metadato geo")
        .value
        .clone()
        .unwrap();
    serde_json::from_str(&voce).unwrap()
}

/// File Parquet costruito a mano: colonna `id`, colonna WKB `geometry`
/// senza metadati di campo, metadato di file `geo` dato.
fn file_a_mano(dir: &Path, geo: &Value, celle: Vec<Option<Vec<u8>>>) -> PathBuf {
    let n = celle.len();
    let celle: BinaryArray = celle.iter().map(|c| c.as_deref()).collect();
    file_con_colonna(dir, &geo.to_string(), Arc::new(celle), n)
}

fn file_con_colonna(dir: &Path, geo: &str, colonna: ArrayRef, n: usize) -> PathBuf {
    let schema: SchemaRef = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("geometry", colonna.data_type().clone(), true),
    ]));
    let ids: Vec<i64> = (0..i64::try_from(n).unwrap()).collect();
    let tabella = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from(ids)), colonna],
    )
    .unwrap();
    let percorso = dir.join("a_mano.parquet");
    let proprieta = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_owned(), geo.to_owned())]))
        .build();
    let mut scrittore =
        ArrowWriter::try_new(File::create(&percorso).unwrap(), schema, Some(proprieta)).unwrap();
    scrittore.write(&tabella).unwrap();
    scrittore.close().unwrap();
    percorso
}

fn geo_colonna(colonna: Value) -> Value {
    json!({"version": "1.1.0", "primary_column": "geometry", "columns": {"geometry": colonna}})
}

fn leggi(percorso: &Path) -> Result<RecordBatch, PlenoraError> {
    leggi_tabella(percorso, None, u64::MAX)
}

#[test]
fn fixture_pyarrow_utm32_rifiutata_e_celle_con_definizione_integrata() {
    let error = leggi(&fixture("pyarrow_utm32.parquet")).unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Crs);
    let tabella = comune::utm32_integrata();
    assert_eq!(tabella.num_rows(), 4);
    assert!(!tabella.schema().metadata().contains_key(GEO_METADATA_KEY));
    assert_eq!(
        tabella
            .schema()
            .metadata()
            .get(PLENORA_CONTRACT_VERSION_KEY)
            .map(String::as_str),
        Some("1")
    );
    let campo = tabella
        .schema()
        .field_with_name("geometry")
        .unwrap()
        .clone();
    assert_eq!(campo.data_type(), &DataType::Binary);
    assert_eq!(
        campo
            .metadata()
            .get(GEOARROW_EXTENSION_KEY)
            .map(String::as_str),
        Some(GEOARROW_WKB_EXTENSION)
    );
    assert_eq!(
        campo
            .metadata()
            .get(PLENORA_GEOMETRY_CRS_ID_KEY)
            .map(String::as_str),
        Some("EPSG:32632")
    );
    assert_eq!(
        campo
            .metadata()
            .get(PLENORA_GEOMETRY_AXIS_ORDER_KEY)
            .map(String::as_str),
        Some("easting_northing")
    );
    assert_eq!(
        campo
            .metadata()
            .get(PLENORA_GEOMETRY_ENCODING_KEY)
            .map(String::as_str),
        Some("wkb")
    );
    assert_eq!(
        campo
            .metadata()
            .get(PLENORA_GEOMETRY_TYPES_DECLARATION_KEY)
            .map(String::as_str),
        Some("exact")
    );
    assert_eq!(
        campo
            .metadata()
            .get(PLENORA_GEOMETRY_TYPES_KEY)
            .map(String::as_str),
        Some("point,polygon")
    );
    let contratto = contratto(&tabella);
    let geometria = &contratto.geometries[0];
    assert_eq!(identificativo(&geometria.crs), "EPSG:32632");
    assert_eq!(geometria.dimensions, GeometryDimensions::Xy);
    assert_eq!(
        geometria.types.value().unwrap().types(),
        &[GeometryType::Point, GeometryType::Polygon]
    );
    let celle = tabella
        .column(2)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert!(celle.is_null(2));
    assert_eq!(celle.value(0), punto(500_000.0, 5_000_000.0).as_slice());
}

#[test]
fn legge_il_file_di_pyarrow_crs84_z() {
    let tabella = leggi(&fixture("pyarrow_crs84_z.parquet")).expect("lettura");
    let contratto = contratto(&tabella);
    let geometria = &contratto.geometries[0];
    assert_eq!(geometria.name, "geom");
    assert_eq!(identificativo(&geometria.crs), "OGC:CRS84");
    assert_eq!(geometria.dimensions, GeometryDimensions::Xyz);
    let valori = tabella
        .column(0)
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::Float64Array>()
        .unwrap();
    assert_eq!(valori.value(1).to_bits(), (-0.0_f64).to_bits());
}

fn tabella_geo(campo_geometria: Field, celle: Vec<Option<Vec<u8>>>) -> RecordBatch {
    // Le fixture di scrittura sono costruite in easting/northing.
    let mut metadata = campo_geometria.metadata().clone();
    if !metadata.contains_key("plenora.geometry.axis_order") {
        metadata.insert(
            "plenora.geometry.axis_order".to_owned(),
            "easting_northing".to_owned(),
        );
    }
    let campo_geometria = campo_geometria.with_metadata(metadata);
    tabella_geo_con_assi_dichiarati(campo_geometria, celle)
}

fn tabella_geo_con_assi_dichiarati(
    campo_geometria: Field,
    celle: Vec<Option<Vec<u8>>>,
) -> RecordBatch {
    let n = celle.len();
    let schema = Arc::new(Schema::new_with_metadata(
        vec![Field::new("id", DataType::Int64, false), campo_geometria],
        HashMap::from([("plenora.contract.version".to_owned(), "1".to_owned())]),
    ));
    let celle: BinaryArray = celle.iter().map(|c| c.as_deref()).collect();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(
                (0..i64::try_from(n).unwrap()).collect::<Vec<_>>(),
            )),
            Arc::new(celle),
        ],
    )
    .unwrap()
}

#[test]
fn scrive_geoparquet_valido_e_lo_rilegge() {
    let tabella = tabella_geo(
        geometry_output_field("geometry", "EPSG:32632").unwrap(),
        vec![
            Some(punto(500_000.0, 5_000_000.0)),
            None,
            Some(linea(&[
                (499_990.5, 5_000_100.0),
                (500_010.0, 4_999_000.25),
            ])),
            Some(punto(f64::NAN, f64::NAN)),
        ],
    );
    let dir = cartella();
    let percorso = dir.path().join("g.parquet");
    scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
    let geo = geo_del_file(&percorso);
    assert_eq!(geo["version"], "1.1.0");
    assert_eq!(geo["primary_column"], "geometry");
    let colonna = &geo["columns"]["geometry"];
    assert_eq!(colonna["encoding"], "WKB");
    assert_eq!(colonna["geometry_types"], json!(["Point", "LineString"]));
    assert_eq!(
        colonna["bbox"],
        json!([499_990.5, 4_999_000.25, 500_010.0, 5_000_100.0])
    );
    assert_eq!(colonna["crs"], projjson_di("EPSG:32632").unwrap());
    assert_eq!(colonna["crs"]["type"], "ProjectedCRS");
    assert!(colonna["crs"]["base_crs"].is_object(), "PROJJSON completo");
    assert!(colonna.get("edges").is_none());
    // Lo schema Arrow incorporato non porta il `geo` di campo.
    let costruttore =
        ParquetRecordBatchReaderBuilder::try_new(File::open(&percorso).unwrap()).unwrap();
    let campo = costruttore
        .schema()
        .field_with_name("geometry")
        .unwrap()
        .clone();
    assert!(!campo.metadata().contains_key(GEO_METADATA_KEY));

    let letta = leggi(&percorso).expect("rilettura");
    assert_eq!(letta.columns(), tabella.columns());
    let prima = contratto(&tabella);
    let dopo = contratto(&letta);
    assert!(prima.geometries[0]
        .crs
        .as_resolved()
        .unwrap()
        .semantically_equals(dopo.geometries[0].crs.as_resolved().unwrap()));
    assert_eq!(dopo.geometries[0].dimensions, GeometryDimensions::Xy);
    assert_eq!(
        dopo.geometries[0].types.value().unwrap().types(),
        &[GeometryType::Point, GeometryType::LineString]
    );

    // Seconda andata e ritorno: stesso schema, stessi byte.
    let percorso2 = dir.path().join("g2.parquet");
    scrivi_tabella(&letta, &percorso2, &OpzioniScrittura::default()).unwrap();
    let riletta = leggi(&percorso2).unwrap();
    assert_eq!(riletta, letta);
    let percorso3 = dir.path().join("g3.parquet");
    scrivi_tabella(&riletta, &percorso3, &OpzioniScrittura::default()).unwrap();
    assert_eq!(
        std::fs::read(&percorso2).unwrap(),
        std::fs::read(&percorso3).unwrap()
    );
}

#[test]
fn crs_assente_e_null() {
    let dir = cartella();
    // Assente: OGC:CRS84.
    let percorso = file_a_mano(
        dir.path(),
        &geo_colonna(json!({"encoding": "WKB", "geometry_types": []})),
        vec![Some(punto(12.0, 42.0))],
    );
    let tabella = leggi(&percorso).unwrap();
    let c = contratto(&tabella);
    assert_eq!(identificativo(&c.geometries[0].crs), "OGC:CRS84");
    assert_eq!(c.geometries[0].dimensions, GeometryDimensions::Unknown);
    assert!(c.geometries[0].types.value().is_none());
    // null: CRS mancante, e si riscrive null.
    let percorso = file_a_mano(
        dir.path(),
        &geo_colonna(json!({"encoding": "WKB", "geometry_types": ["Point"], "crs": null})),
        vec![Some(punto(1.0, 2.0))],
    );
    let tabella = leggi(&percorso).unwrap();
    assert!(matches!(
        contratto(&tabella).geometries[0].crs,
        ContractCrs::Missing
    ));
    assert_eq!(
        tabella
            .schema()
            .field(1)
            .metadata()
            .get(PLENORA_GEOMETRY_CRS_RESOLUTION_KEY)
            .map(String::as_str),
        Some("missing")
    );
    let uscita = dir.path().join("null.parquet");
    // Il vocabolario missing non porta axis_order: la scrittura non pu?
    // certificare l'ordine da un CRS assente (migrazione 4.0).
    let error = scrivi_tabella(&tabella, &uscita, &OpzioniScrittura::default()).unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Crs);
    assert!(!uscita.exists());
}

#[test]
fn crs_epsg_32632_e_crs84_da_projjson() {
    let dir = cartella();
    for (identificativo_atteso, cella) in [
        ("EPSG:32632", punto(500_000.0, 5_000_000.0)),
        ("OGC:CRS84", punto(12.0, 42.0)),
        ("EPSG:4326", punto(12.0, 42.0)),
    ] {
        let percorso = file_a_mano(
            dir.path(),
            &geo_colonna(json!({
                "encoding": "WKB",
                "geometry_types": ["Point"],
                "crs": projjson_di(identificativo_atteso).unwrap(),
                "edges": "planar",
                "orientation": "counterclockwise",
                "bbox": [0, 0, 1, 1],
                "covering": {"bbox": {"xmin": ["bbox", "xmin"]}}
            })),
            vec![Some(cella)],
        );
        let tabella = leggi(&percorso).unwrap();
        assert_eq!(
            identificativo(&contratto(&tabella).geometries[0].crs),
            identificativo_atteso
        );
        // Assi: sempre x = est/longitudine (GeoParquet), anche per 4326.
        let asse = tabella
            .schema()
            .field(1)
            .metadata()
            .get(PLENORA_GEOMETRY_AXIS_ORDER_KEY)
            .cloned();
        let atteso = if identificativo_atteso == "EPSG:32632" {
            "easting_northing"
        } else {
            "lon_lat"
        };
        assert_eq!(asse.as_deref(), Some(atteso));
    }
}

fn errore_di(geo: &Value, celle: Vec<Option<Vec<u8>>>) -> PlenoraError {
    let dir = cartella();
    let percorso = file_a_mano(dir.path(), geo, celle);
    leggi(&percorso).expect_err("rifiutato")
}

/// Un caso di rifiuto: nome, metadato `geo`, celle, categoria, frammento
/// atteso nel messaggio.
type Caso = (
    &'static str,
    Value,
    Vec<Option<Vec<u8>>>,
    ErrorCategory,
    &'static str,
);

#[test]
#[allow(clippy::too_many_lines)] // Un caso per riga di tabella.
fn rifiuti_in_lettura() {
    let p = || vec![Some(punto(1.0, 2.0))];
    let colonna = |extra: Value| {
        let mut base = json!({"encoding": "WKB", "geometry_types": ["Point"]});
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        geo_colonna(base)
    };
    let casi: Vec<Caso> = vec![
        (
            "CRS fuori tabella",
            colonna(
                json!({"crs": {"type": "ProjectedCRS", "id": {"authority": "EPSG", "code": 99_999}}}),
            ),
            p(),
            ErrorCategory::Crs,
            "CRS_NOT_BUILTIN",
        ),
        (
            "autorita' ESRI",
            colonna(
                json!({"crs": {"type": "ProjectedCRS", "id": {"authority": "ESRI", "code": 102_100}}}),
            ),
            p(),
            ErrorCategory::Crs,
            "CRS_NOT_BUILTIN",
        ),
        (
            "PROJJSON senza id",
            colonna(json!({"crs": {"type": "GeographicCRS", "name": "WGS 84"}})),
            p(),
            ErrorCategory::Crs,
            "CRS_NOT_BUILTIN",
        ),
        (
            "crs testuale",
            colonna(json!({"crs": "EPSG:4326"})),
            p(),
            ErrorCategory::InvalidPlan,
            "PROJJSON",
        ),
        (
            "edges spherical",
            colonna(json!({"edges": "spherical"})),
            p(),
            ErrorCategory::Unsupported,
            "spherical",
        ),
        (
            "edges sconosciuto",
            colonna(json!({"edges": "curvi"})),
            p(),
            ErrorCategory::InvalidPlan,
            "edges",
        ),
        (
            "epoch",
            colonna(json!({"epoch": 2021.5})),
            p(),
            ErrorCategory::Unsupported,
            "epoch",
        ),
        (
            "encoding nativa",
            colonna(json!({"encoding": "point"})),
            p(),
            ErrorCategory::Unsupported,
            "nativa",
        ),
        (
            "encoding sconosciuta",
            colonna(json!({"encoding": "wkt"})),
            p(),
            ErrorCategory::InvalidPlan,
            "encoding",
        ),
        (
            "chiave sconosciuta",
            colonna(json!({"futura": 1})),
            p(),
            ErrorCategory::Unsupported,
            "futura",
        ),
        (
            "tipo ripetuto",
            colonna(json!({"geometry_types": ["Point", "Point"]})),
            p(),
            ErrorCategory::InvalidPlan,
            "ripetuto",
        ),
        (
            "tipo M",
            colonna(json!({"geometry_types": ["Point M"]})),
            p(),
            ErrorCategory::InvalidPlan,
            "specifica",
        ),
        (
            "tipi non veri",
            colonna(json!({"geometry_types": ["Polygon"]})),
            p(),
            ErrorCategory::DataMapping,
            "geometry_types",
        ),
        (
            "Z dichiarata, dati 2D",
            colonna(json!({"geometry_types": ["Point Z"]})),
            p(),
            ErrorCategory::DataMapping,
            "geometry_types",
        ),
        (
            "bbox corto",
            colonna(json!({"bbox": [0, 0, 1]})),
            p(),
            ErrorCategory::InvalidPlan,
            "bbox",
        ),
        (
            "orientation",
            colonna(json!({"orientation": "clockwise"})),
            p(),
            ErrorCategory::InvalidPlan,
            "orientation",
        ),
        (
            "versione",
            json!({"version": "2.0.0", "primary_column": "geometry", "columns": {"geometry": {"encoding": "WKB", "geometry_types": []}}}),
            p(),
            ErrorCategory::Unsupported,
            "versione",
        ),
        (
            "primaria assente",
            json!({"version": "1.1.0", "primary_column": "altra", "columns": {"geometry": {"encoding": "WKB", "geometry_types": []}}}),
            p(),
            ErrorCategory::InvalidPlan,
            "primary_column",
        ),
        (
            "colonna assente",
            json!({"version": "1.1.0", "primary_column": "geometry", "columns": {"geometry": {"encoding": "WKB", "geometry_types": []}, "manca": {"encoding": "WKB", "geometry_types": []}}}),
            p(),
            ErrorCategory::InvalidPlan,
            "assente",
        ),
        (
            "colonna non binaria",
            json!({"version": "1.1.0", "primary_column": "id", "columns": {"id": {"encoding": "WKB", "geometry_types": []}}}),
            p(),
            ErrorCategory::Unsupported,
            "Binary",
        ),
        (
            "EWKB",
            colonna(json!({})),
            vec![Some({
                let mut c = punto(1.0, 2.0);
                c[1..5].copy_from_slice(&0x2000_0001_u32.to_le_bytes());
                c
            })],
            ErrorCategory::DataMapping,
            "EWKB",
        ),
        (
            "M nei dati",
            colonna(json!({"geometry_types": []})),
            vec![Some({
                let mut c = punto(1.0, 2.0);
                c[1..5].copy_from_slice(&2001_u32.to_le_bytes());
                c.extend_from_slice(&0.0_f64.to_le_bytes());
                c
            })],
            ErrorCategory::DataMapping,
            "M",
        ),
    ];
    for (nome, geo, celle, categoria, frammento) in casi {
        let errore = errore_di(&geo, celle);
        assert_eq!(errore.category(), categoria, "{nome}: {errore}");
        assert!(errore.to_string().contains(frammento), "{nome}: {errore}");
        // Errori senza dati: il valore letto dal metadato non compare.
        assert!(!errore.to_string().contains("2.0.0"), "{nome}: {errore}");
    }
    // Chiavi duplicate nel testo.
    let dir = cartella();
    let testo = r#"{"version":"1.1.0","version":"1.0.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":[]}}}"#;
    let wkb = punto(1.0, 2.0);
    let celle = BinaryArray::from(vec![Some(wkb.as_slice())]);
    let percorso = file_con_colonna(dir.path(), testo, Arc::new(celle), 1);
    let errore = leggi(&percorso).expect_err("duplicate");
    assert!(errore.to_string().contains("duplicate"), "{errore}");
}

#[test]
fn una_sola_colonna_geometrica() {
    // Il contratto v1 ammette una sola colonna geometrica: un file con
    // due si rifiuta, qualunque sia la primaria.
    let dir = cartella();
    let schema: SchemaRef = Arc::new(Schema::new(vec![
        Field::new("a", DataType::Binary, true),
        Field::new("b", DataType::Binary, true),
    ]));
    let celle =
        || -> ArrayRef { Arc::new(BinaryArray::from(vec![Some(punto(1.0, 1.0).as_slice())])) };
    let tabella = RecordBatch::try_new(schema.clone(), vec![celle(), celle()]).unwrap();
    for (primaria, categoria) in [
        ("a", ErrorCategory::Unsupported),
        ("b", ErrorCategory::Unsupported),
    ] {
        let geo = json!({"version": "1.1.0", "primary_column": primaria, "columns": {
            "a": {"encoding": "WKB", "geometry_types": ["Point"]},
            "b": {"encoding": "WKB", "geometry_types": ["Point"], "crs": null}}});
        let percorso = dir.path().join(format!("{primaria}.parquet"));
        let proprieta = WriterProperties::builder()
            .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_owned(), geo.to_string())]))
            .build();
        let mut w = ArrowWriter::try_new(
            File::create(&percorso).unwrap(),
            schema.clone(),
            Some(proprieta),
        )
        .unwrap();
        w.write(&tabella).unwrap();
        w.close().unwrap();
        let errore = leggi(&percorso).expect_err("due geometrie");
        assert_eq!(errore.category(), categoria, "{primaria}: {errore}");
    }
}

#[test]
fn large_binary_si_converte() {
    let dir = cartella();
    let celle: LargeBinaryArray = [Some(punto(1.0, 2.0)), None]
        .iter()
        .map(|c| c.as_deref())
        .collect();
    let percorso = file_con_colonna(
        dir.path(),
        &geo_colonna(json!({"encoding": "WKB", "geometry_types": ["Point"]})).to_string(),
        Arc::new(celle),
        2,
    );
    let tabella = leggi(&percorso).unwrap();
    assert_eq!(tabella.schema().field(1).data_type(), &DataType::Binary);
    let celle = tabella
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .unwrap();
    assert_eq!(celle.value(0), punto(1.0, 2.0).as_slice());
    assert!(celle.is_null(1));
}

fn errore_scrittura(campo: Field, celle: Vec<Option<Vec<u8>>>) -> PlenoraError {
    let tabella = tabella_geo(campo, celle);
    let elenco = tabella.schema().fields().clone();
    let tabella = tabella
        .with_schema(Arc::new(Schema::new_with_metadata(
            elenco,
            HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1".to_owned())]),
        )))
        .unwrap();
    let dir = cartella();
    let percorso = dir.path().join("x.parquet");
    let errore =
        scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).expect_err("rifiutata");
    assert!(!percorso.exists());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "temporaneo rimasto"
    );
    errore
}

fn con_metadati(campo: Field, coppie: &[(&str, &str)]) -> Field {
    let mut metadati = campo.metadata().clone();
    for (k, v) in coppie {
        metadati.insert((*k).to_owned(), (*v).to_owned());
    }
    campo.with_metadata(metadati)
}

#[test]
fn rifiuti_in_scrittura() {
    let base = || geometry_output_field("geometry", "EPSG:32632").unwrap();
    let p = || vec![Some(punto(500_000.0, 5_000_000.0))];
    // Ordine degli assi nord-est.
    let errore = errore_scrittura(
        con_metadati(
            base(),
            &[(PLENORA_GEOMETRY_AXIS_ORDER_KEY, "northing_easting")],
        ),
        p(),
    );
    assert_eq!(errore.category(), ErrorCategory::Crs, "{errore}");
    // Encoding EWKB dichiarato.
    let errore = errore_scrittura(
        con_metadati(base(), &[(PLENORA_GEOMETRY_ENCODING_KEY, "ewkb")]),
        p(),
    );
    assert!(
        errore.to_string().contains("EWKB") || errore.to_string().contains("ewkb"),
        "{errore}"
    );
    // Tipi dichiarati dal contratto e dati di un altro tipo.
    let errore = errore_scrittura(
        con_metadati(
            base(),
            &[
                (PLENORA_GEOMETRY_TYPES_DECLARATION_KEY, "exact"),
                (PLENORA_GEOMETRY_TYPES_KEY, "polygon"),
            ],
        ),
        p(),
    );
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    // Dimensionalita' dichiarata xyz, dati 2D.
    let errore = errore_scrittura(
        geometry_output_field_with_dimensions("geometry", "EPSG:32632", GeometryDimensions::Xyz)
            .unwrap(),
        p(),
    );
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    // CRS dichiarato non risolto.
    let campo = Field::new("geometry", DataType::Binary, true).with_metadata(HashMap::from([
        ("ARROW:extension:name".to_owned(), "geoarrow.wkb".to_owned()),
        (PLENORA_GEOMETRY_ENCODING_KEY.to_owned(), "wkb".to_owned()),
        ("plenora.geometry.dimensions".to_owned(), "xy".to_owned()),
        (
            PLENORA_GEOMETRY_CRS_RESOLUTION_KEY.to_owned(),
            "declared_unresolved".to_owned(),
        ),
        (
            PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(),
            "EPSG:32632".to_owned(),
        ),
        (
            PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
            "easting_northing".to_owned(),
        ),
    ]));
    let tabella = tabella_geo(campo, p());
    let elenco = tabella.schema().fields().clone();
    let tabella = tabella
        .with_schema(Arc::new(Schema::new_with_metadata(
            elenco,
            HashMap::from([(PLENORA_CONTRACT_VERSION_KEY.to_owned(), "1".to_owned())]),
        )))
        .unwrap();
    let dir = cartella();
    let errore = scrivi_tabella(
        &tabella,
        &dir.path().join("u.parquet"),
        &OpzioniScrittura::default(),
    )
    .expect_err("non risolto");
    assert_eq!(errore.category(), ErrorCategory::Crs, "{errore}");
    // Curva nei dati.
    let mut curva = linea(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)]);
    curva[1..5].copy_from_slice(&8_u32.to_le_bytes());
    let errore = errore_scrittura(base(), vec![Some(curva)]);
    assert_eq!(errore.category(), ErrorCategory::DataMapping, "{errore}");
    // Metadato di schema `geo` senza geometrie.
    let tabella = comune::ordini();
    let elenco = tabella.schema().fields().clone();
    let tabella = tabella
        .with_schema(Arc::new(Schema::new_with_metadata(
            elenco,
            HashMap::from([("geo".to_owned(), "{}".to_owned())]),
        )))
        .unwrap();
    let errore = scrivi_tabella(
        &tabella,
        &dir.path().join("s.parquet"),
        &OpzioniScrittura::default(),
    )
    .expect_err("geo riservato");
    assert_eq!(errore.category(), ErrorCategory::InvalidPlan);
}

#[test]
fn z_si_scrive_senza_bbox() {
    let tabella = leggi(&fixture("pyarrow_crs84_z.parquet")).unwrap();
    let dir = cartella();
    let percorso = dir.path().join("z.parquet");
    scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
    let geo = geo_del_file(&percorso);
    assert_eq!(geo["columns"]["geom"]["geometry_types"], json!(["Point Z"]));
    assert!(geo["columns"]["geom"].get("bbox").is_none());
    assert_eq!(
        geo["columns"]["geom"]["crs"]["id"],
        json!({"authority": "OGC", "code": "CRS84"})
    );
    assert_eq!(leggi(&percorso).unwrap(), tabella);
}

#[test]
fn la_geometria_resta_in_ipc_con_il_suo_contratto() {
    let tabella = comune::utm32_integrata();
    let dir = cartella();
    let percorso = dir.path().join("g.arrow");
    scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
    let letta = leggi(&percorso).unwrap();
    assert_eq!(letta, tabella);
    assert_eq!(letta.schema(), tabella.schema());
    let uscita = dir.path().join("da_ipc.parquet");
    scrivi_tabella(&letta, &uscita, &OpzioniScrittura::default()).unwrap();
    assert_eq!(
        geo_del_file(&uscita)["columns"]["geometry"]["geometry_types"],
        json!(["Point", "Polygon"])
    );
    let _: &dyn Array = tabella.column(0).as_ref();
}

/// Andata e ritorno di tabelle nella forma canonica del contratto (quella
/// che il runner emette): ogni combinazione di dimensionalita', dati e
/// dichiarazione dei tipi si rilegge, con la stessa dimensionalita' e la
/// stessa dichiarazione quando c'era.
#[test]
#[allow(clippy::too_many_lines)] // La matrice dei casi in un posto solo.
fn forma_canonica_andata_e_ritorno() {
    use plenora_core::contract::arrow_schema::arrow_schema_from_contract;
    use plenora_core::contract::{
        ContractProperty, GeometryTypesProperty, PropertyConfidence, PropertyScope,
        TypesDeclaration,
    };

    fn punto_z(x: f64, y: f64) -> Vec<u8> {
        let mut v = vec![1_u8];
        v.extend_from_slice(&1001_u32.to_le_bytes());
        for c in [x, y, 5.0] {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v
    }

    let dichiarazioni: Vec<Option<GeometryTypesProperty>> = vec![
        None,
        Some(GeometryTypesProperty::new(TypesDeclaration::Unresolved, vec![]).unwrap()),
        Some(
            GeometryTypesProperty::new(
                TypesDeclaration::Mixed,
                vec![GeometryType::Point, GeometryType::Polygon],
            )
            .unwrap(),
        ),
        Some(
            GeometryTypesProperty::new(TypesDeclaration::Exact, vec![GeometryType::Point]).unwrap(),
        ),
    ];
    let mut casi = 0;
    for dimensioni in [
        GeometryDimensions::Unknown,
        GeometryDimensions::Xy,
        GeometryDimensions::Xyz,
    ] {
        let cella = |x: f64| {
            if dimensioni == GeometryDimensions::Xyz {
                punto_z(x, 5_000_000.0)
            } else {
                punto(x, 5_000_000.0)
            }
        };
        let dati: Vec<Vec<Option<Vec<u8>>>> = vec![
            vec![Some(cella(500_000.0)), None, Some(cella(500_001.0))],
            vec![],
            vec![None, None],
        ];
        for celle in dati {
            for dichiarazione in &dichiarazioni {
                let campo =
                    geometry_output_field_with_dimensions("geometry", "EPSG:32632", dimensioni)
                        .unwrap();
                let tabella = tabella_geo(campo, celle.clone());
                let mut c = contratto(&tabella);
                if let Some(tipi) = dichiarazione {
                    c.geometries[0].types = ContractProperty::new(
                        PropertyConfidence::Declared(tipi.clone()),
                        PropertyScope::Schema,
                    );
                }
                let schema = arrow_schema_from_contract(&c).unwrap();
                let tabella = tabella.with_schema(schema).unwrap();
                let dir = cartella();
                let percorso = dir.path().join("c.parquet");
                scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default())
                    .unwrap_or_else(|e| panic!("{dimensioni:?} {dichiarazione:?}: {e}"));
                let letta = leggi(&percorso).unwrap_or_else(|e| {
                    panic!(
                        "rilettura {dimensioni:?} {dichiarazione:?} {} celle: {e}",
                        celle.len()
                    )
                });
                assert_eq!(letta.columns(), tabella.columns());
                let dopo = contratto(&letta);
                let g = &dopo.geometries[0];
                assert_eq!(g.dimensions, dimensioni, "{dichiarazione:?}");
                // `unresolved` e la proprieta' non dichiarata dicono la
                // stessa cosa: la lettura le unifica, e lo scrittore scrive i
                // tipi dei dati per entrambe.
                let dichiarata = dichiarazione
                    .as_ref()
                    .filter(|tipi| tipi.declaration() != TypesDeclaration::Unresolved);
                if let Some(tipi) = dichiarata {
                    assert_eq!(g.types.value(), Some(tipi), "{dimensioni:?}");
                } else {
                    // Tipi dai dati solo con dimensionalita' nota e geometrie
                    // presenti; altrimenti restano non dichiarati.
                    let attesi = dimensioni != GeometryDimensions::Unknown
                        && celle.iter().any(Option::is_some);
                    assert_eq!(g.types.value().is_some(), attesi, "{dimensioni:?}");
                }
                // Seconda andata e ritorno: stesso schema.
                let percorso2 = dir.path().join("c2.parquet");
                scrivi_tabella(&letta, &percorso2, &OpzioniScrittura::default()).unwrap();
                assert_eq!(leggi(&percorso2).unwrap(), letta);
                casi += 1;
            }
        }
    }
    assert_eq!(casi, 36);
}

/// Senza blocco canonico, zero geometrie e tipi non dichiarati:
/// `geometry_types` vuoto, dimensionalita' riletta `unknown` (limite
/// dichiarato: `GeoParquet` non la rappresenta senza tipi).
#[test]
fn senza_geometrie_la_dimensionalita_non_si_rappresenta() {
    let tabella = tabella_geo(
        geometry_output_field("geometry", "EPSG:32632").unwrap(),
        vec![None],
    );
    let dir = cartella();
    let percorso = dir.path().join("vuota.parquet");
    scrivi_tabella(&tabella, &percorso, &OpzioniScrittura::default()).unwrap();
    assert_eq!(
        geo_del_file(&percorso)["columns"]["geometry"]["geometry_types"],
        json!([])
    );
    assert!(geo_del_file(&percorso)["columns"]["geometry"]
        .get("bbox")
        .is_none());
    let letta = leggi(&percorso).unwrap();
    assert_eq!(
        contratto(&letta).geometries[0].dimensions,
        GeometryDimensions::Unknown
    );
}

#[test]
fn scrittura_non_certifica_assi_ambigui_o_incompatibili() {
    let base = tabella_geo(
        geometry_output_field("geometry", "EPSG:32632").unwrap(),
        vec![Some(punto(500_000.0, 5_000_000.0))],
    );
    for axis in [
        None,
        Some("unknown"),
        Some("lon_lat"),
        Some("lat_lon"),
        Some("northing_easting"),
    ] {
        let schema = base.schema();
        let mut metadata = schema.field(1).metadata().clone();
        metadata.remove("plenora.geometry.axis_order");
        if let Some(axis) = axis {
            metadata.insert("plenora.geometry.axis_order".to_owned(), axis.to_owned());
        }
        let fields = vec![
            schema.field(0).clone(),
            schema.field(1).clone().with_metadata(metadata),
        ];
        let table = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
            base.columns().to_vec(),
        )
        .unwrap();
        let dir = cartella();
        let output = dir.path().join("refused.parquet");
        let error = scrivi_tabella(&table, &output, &OpzioniScrittura::default()).unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Crs);
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn provenienza_geoparquet_non_sovrascrive_versioni_non_supportate() {
    let base = tabella_geo(
        geometry_output_field("geometry", "EPSG:32632").unwrap(),
        vec![Some(punto(500_000.0, 5_000_000.0))],
    );
    let metadata =
        geo_colonna(json!({"encoding":"WKB","geometry_types":["Point"],"crs":projjson_di("EPSG:32632").unwrap()})).to_string();
    plenora_io::geoparquet::applica(&base, &metadata).unwrap();
    for (version, category) in [
        ("2", ErrorCategory::Unsupported),
        ("illeggibile", ErrorCategory::Schema),
    ] {
        let schema = base.schema();
        let mut md = schema.metadata().clone();
        md.insert("plenora.contract.version".to_owned(), version.to_owned());
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(schema.fields().clone(), md)),
            base.columns().to_vec(),
        )
        .unwrap();
        assert_eq!(
            plenora_io::geoparquet::applica(&batch, &metadata)
                .unwrap_err()
                .category(),
            category
        );
    }
}

#[test]
fn assi_geoparquet_verificati_in_ogni_ramo_crs() {
    for id in [
        None,
        Some("OGC:CRS84"),
        Some("EPSG:4326"),
        Some("EPSG:32632"),
    ] {
        for null in [false, true] {
            let mut colonna = json!({"encoding":"WKB", "geometry_types":["Point"]});
            if null {
                colonna["crs"] = Value::Null;
            } else if let Some(id) = id {
                colonna["crs"] = projjson_di(id).unwrap();
            }
            let atteso = if id == Some("EPSG:32632") {
                "easting_northing"
            } else {
                "lon_lat"
            };
            for ordine in [
                None,
                Some("unknown"),
                Some("lon_lat"),
                Some("lat_lon"),
                Some("easting_northing"),
                Some("northing_easting"),
            ] {
                let mut metadata = HashMap::new();
                if let Some(ordine) = ordine {
                    metadata.insert(
                        PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
                        ordine.to_owned(),
                    );
                }
                // Con CRS null anche un CRS canonico residuo non deve aggirare il rifiuto.
                if let Some(id) = id {
                    metadata.insert(PLENORA_GEOMETRY_CRS_ID_KEY.to_owned(), id.to_owned());
                }
                let batch = tabella_geo_con_assi_dichiarati(
                    Field::new("geometry", DataType::Binary, true).with_metadata(metadata),
                    vec![Some(punto(12.0, 45.0))],
                );
                let risultato = plenora_io::geoparquet::applica(
                    &batch,
                    &geo_colonna(colonna.clone()).to_string(),
                );
                let incompatibile = (null && (id.is_some() || ordine.is_some()))
                    || matches!(ordine, Some("lat_lon" | "northing_easting"))
                    || (!null && ordine.is_some_and(|o| o != "unknown" && o != atteso));
                if incompatibile {
                    assert_eq!(
                        risultato.unwrap_err().category(),
                        ErrorCategory::Crs,
                        "{id:?} {null} {ordine:?}"
                    );
                } else {
                    let letta = risultato.unwrap();
                    assert_eq!(letta.columns(), batch.columns());
                }
            }
        }
    }
}

#[test]
fn assi_geoparquet_precedono_la_lettura_delle_celle() {
    let geo = geo_colonna(json!({"encoding":"WKB", "geometry_types":["Point"],
        "crs":projjson_di("EPSG:3857").unwrap()}))
    .to_string();
    for ordine in ["northing_easting", "lat_lon", "easting_northing"] {
        let campo = geometry_output_field("geometry", "EPSG:3857").unwrap();
        let mut md = campo.metadata().clone();
        md.insert(
            PLENORA_GEOMETRY_AXIS_ORDER_KEY.to_owned(),
            ordine.to_owned(),
        );
        let batch = tabella_geo(campo.with_metadata(md), vec![Some(vec![1_u8, 2, 3])]);
        let errore = plenora_io::geoparquet::applica(&batch, &geo).unwrap_err();
        if ordine == "easting_northing" {
            assert_ne!(errore.category(), ErrorCategory::Crs);
        } else {
            assert_eq!(errore.category(), ErrorCategory::Crs);
        }
    }
}

#[test]
fn geoparquet_non_sovrascrive_un_crs_legacy_diverso() {
    let campo = geometry_output_field("geometry", "EPSG:4326").unwrap();
    let batch = tabella_geo(campo, vec![Some(punto(12.0, 45.0))]);
    for crs in [Value::Null, projjson_di("EPSG:3857").unwrap()] {
        let metadata =
            geo_colonna(json!({"encoding":"WKB", "geometry_types":["Point"], "crs":crs}))
                .to_string();
        for input in [batch.clone(), batch.slice(0, 0)] {
            assert_eq!(
                plenora_io::geoparquet::applica(&input, &metadata)
                    .unwrap_err()
                    .category(),
                ErrorCategory::Crs
            );
        }
    }
}

#[test]
fn geoparquet_non_cancella_dichiarazioni_estensione_non_supportate() {
    for extension in [
        json!({"crs":projjson_di("EPSG:4326").unwrap()}),
        json!({"edges":"spherical"}),
    ] {
        let campo =
            Field::new("geometry", DataType::Binary, true).with_metadata(HashMap::from([(
                "ARROW:extension:metadata".to_owned(),
                extension.to_string(),
            )]));
        let batch = tabella_geo_con_assi_dichiarati(campo, vec![Some(punto(12.0, 45.0))]);
        let file = geo_colonna(json!({"encoding":"WKB", "geometry_types":["Point"], "crs":projjson_di("EPSG:3857").unwrap()})).to_string();
        for input in [batch.clone(), batch.slice(0, 0)] {
            assert_eq!(
                plenora_io::geoparquet::applica(&input, &file)
                    .unwrap_err()
                    .category(),
                ErrorCategory::Unsupported
            );
        }
    }
}

#[test]
fn definizione_projjson_integrata_non_si_deduce_dal_solo_id() {
    let originale = projjson_di("EPSG:32632").unwrap();
    // Ogni componente della definizione deve essere identico, anche quando
    // il codice EPSG resta valido. Le coordinate non sono simmetriche.
    for pointer in [
        "/id/code",
        "/base_crs/datum_ensemble/ellipsoid/semi_major_axis",
        "/conversion/method/name",
        "/conversion/parameters/1/value",
        "/coordinate_system/axis/0/unit",
        "/coordinate_system/axis/0/direction",
    ] {
        let mut documento = originale.clone();
        let target = documento
            .pointer_mut(pointer)
            .expect("componente integrata");
        *target = if pointer == "/id/code" {
            json!(32633)
        } else {
            json!("SENTINELLA_CRS")
        };
        for cells in [
            vec![Some(punto(500_000.0, 4_982_950.0))],
            vec![None],
            vec![],
        ] {
            let error = errore_di(
                &geo_colonna(
                    json!({"encoding":"WKB", "geometry_types":["Point"], "crs":documento}),
                ),
                cells,
            );
            assert_eq!(error.category(), ErrorCategory::Crs);
            assert!(!error.to_string().contains("SENTINELLA_CRS"));
        }
    }
}

#[test]
fn projjson_confronta_i_byte_prima_della_normalizzazione_json() {
    let dir = cartella();
    let crs = projjson_di("EPSG:32632").unwrap();
    let path = file_a_mano(
        dir.path(),
        &geo_colonna(json!({"encoding":"WKB", "geometry_types":["Point"], "crs":crs})),
        vec![Some(punto(500_000.0, 4_982_950.0))],
    );
    let table = leggi(&path).unwrap();
    let canonical = crs.to_string();
    let geo =
        geo_colonna(json!({"encoding":"WKB", "geometry_types":["Point"], "crs":crs})).to_string();
    for modified in [
        serde_json::to_string_pretty(&crs).unwrap(),
        canonical.replace("6378137", "6378137.00000000000000000001"),
        canonical.replace("WGS", r"W\u0047S"),
    ] {
        assert_ne!(modified, canonical);
        let error = plenora_io::geoparquet::applica(&table, &geo.replace(&canonical, &modified))
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::Crs);
    }
}
