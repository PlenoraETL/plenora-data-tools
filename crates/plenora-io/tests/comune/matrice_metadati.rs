//! Matrice generata: fixture delle schede, pipeline pubblica e file reali.
use super::*;
use geo::BoundingRect;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use plenora_core::arrow::ipc::reader::{FileReader as ArrowFileReader, StreamReader};
use plenora_core::contract::geometry_metadata_policy::{self as policy, Class, Effect};
use serde_json::json;

const ORIGIN: &str = "test.geometry_origin";
const ALL_TYPES: &str =
    "point,linestring,polygon,multipoint,multilinestring,multipolygon,geometrycollection";

#[derive(Clone, Copy, Debug)]
enum Site {
    Field,
    Geo,
    Extension,
    SchemaGeo,
}
impl Site {
    const fn container(self) -> Option<&'static str> {
        match self {
            Self::Field | Self::SchemaGeo => None,
            Self::Geo => Some("geo"),
            Self::Extension => Some("ARROW:extension:metadata"),
        }
    }
}
fn sites(key: &str) -> Vec<Site> {
    if key.starts_with("plenora.") || key.starts_with("ARROW:") {
        vec![Site::Field]
    } else {
        vec![Site::Field, Site::Geo, Site::Extension, Site::SchemaGeo]
    }
}
fn read(field: &Field, key: &str, site: Site) -> Option<Value> {
    site.container().map_or_else(
        || field.metadata().get(key).map(|s| Value::String(s.clone())),
        |container| {
            field
                .metadata()
                .get(container)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|v| v.get(key).cloned())
        },
    )
}
fn read_schema(batch: &RecordBatch, key: &str, name: &str) -> Option<Value> {
    batch
        .schema()
        .metadata()
        .get("geo")
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| {
            v.get("columns")
                .and_then(|v| v.get(name))
                .and_then(|v| v.get(key))
                .cloned()
        })
}
fn canonical(key: &str) -> bool {
    key == "ARROW:extension:name"
        || (key.starts_with("plenora.geometry.") && !key.starts_with("plenora.geometry.native."))
}
fn fixture(original: &RecordBatch, origin: usize, geometry: bool) -> RecordBatch {
    let batch = if geometry && !original.schema().fields().iter().any(|f| e_geometria(f)) {
        ingresso_geometrico(original, original.num_columns(), true)
    } else {
        original.clone()
    };
    let contract = contract_from_arrow_schema(batch.schema(), resolve_crs).unwrap();
    let schema = arrow_schema_from_contract(&contract).unwrap();
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|f| {
            let mut md = f.metadata().clone();
            if e_geometria(f) {
                md.insert(ORIGIN, origin.to_string());
                if let Some(id) = md.get("plenora.geometry.crs_id") {
                    let axis = resolve_crs(id, "crs").unwrap().normalized_gis_axis_order();
                    md.insert("plenora.geometry.axis_order", axis.as_str());
                }
            }
            f.as_ref().clone().with_metadata(md)
        })
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        batch.columns().to_vec(),
    )
    .unwrap()
}
fn sample(key: &str, field: &Field, site: Site, different: bool) -> Value {
    if different {
        let first = sample(key, field, site, false);
        // Anche le dichiarazioni a vocabolario chiuso ricevono un conflitto:
        // il validatore puo' rifiutarlo, ma non si ripete il caso uguale.
        let alternative = match key {
            "plenora.geometry.precision" => Some(json!("float32")),
            "epoch" | "plenora.geometry.native.geoparquet.epoch" => Some(json!(2024.5)),
            "precision" => Some(json!(0.02)),
            "bbox" | "plenora.geometry.native.geoparquet.bbox" => {
                Some(json!([-90_000_000, -90_000_000, 90_000_000, 90_000_000]))
            }
            "edges" | "plenora.geometry.native.geoparquet.edges" => Some(json!("spherical")),
            "plenora.geometry.srid" => Some(json!("100002")),
            _ => None,
        };
        if let Some(value) = alternative {
            return if matches!(site, Site::Field) {
                json!(value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned))
            } else {
                value
            };
        }
        return conflict(first);
    }
    sample_equal(key, field, site)
}
fn conflict(first: Value) -> Value {
    match first {
        Value::String(value) => json!(format!("{value}-conflitto")),
        Value::Number(_) => json!(2024),
        Value::Array(mut value) => {
            value.push(json!("conflitto"));
            Value::Array(value)
        }
        Value::Object(mut value) => {
            value.insert("conflitto".into(), json!(true));
            Value::Object(value)
        }
        _ => json!("conflitto"),
    }
}
fn input_types(batch: &RecordBatch, field: &Field) -> Value {
    let values = batch
        .column_by_name(field.name())
        .unwrap()
        .as_any()
        .downcast_ref::<plenora_core::arrow::array::BinaryArray>()
        .unwrap();
    let mut summary = plenora_io::wkb::Sommario::default();
    for bytes in values.iter().flatten() {
        plenora_io::wkb::scansiona_cella(bytes, &mut summary).unwrap();
    }
    let types = plenora_core::contract::GeometryTypesProperty::new(
        plenora_core::contract::TypesDeclaration::Mixed,
        summary.tipi.into_iter().map(|(kind, _)| kind).collect(),
    )
    .unwrap();
    json!(types.to_canonical_list())
}
enum TypeExpectation {
    Other,
    Value(Option<Value>),
}
fn mapped_types(key: &str, op: &str, source: Option<&Field>) -> TypeExpectation {
    use plenora_core::contract::{
        ContractProperty, GeometryColumnContract, PropertyConfidence, PropertyScope,
    };
    if !matches!(
        key,
        "plenora.geometry.types" | "plenora.geometry.types_declaration"
    ) {
        return TypeExpectation::Other;
    }
    let Some(source) = source else {
        return TypeExpectation::Other;
    };
    let property = plenora_core::contract::arrow_metadata::canonical_geometry_types(source)
        .unwrap()
        .map_or_else(GeometryColumnContract::undeclared_types, |types| {
            ContractProperty::new(PropertyConfidence::Declared(types), PropertyScope::Schema)
        });
    let Some(mapped) = policy::types::tipi_di_uscita(op, &property).unwrap() else {
        return TypeExpectation::Other;
    };
    TypeExpectation::Value(if key == "plenora.geometry.types" {
        mapped
            .value()
            .filter(|t| !t.types().is_empty())
            .map(|t| json!(t.to_canonical_list()))
    } else {
        Some(json!(mapped
            .value()
            .map_or("unresolved", |t| t.declaration().as_str())))
    })
}
fn sample_equal(key: &str, field: &Field, site: Site) -> Value {
    let different = false;
    if let Some(value) = read(field, key, site) {
        if !different && key != "plenora.geometry.precision" {
            return value;
        }
    }
    let crs = field
        .metadata()
        .get("plenora.geometry.crs_id")
        .map_or("EPSG:32632", String::as_str);
    let value = match key {
        "crs" if matches!(site, Site::Extension) => {
            plenora_io::crs_projjson::projjson_di(crs).unwrap()
        }
        "crs" | "plenora.geometry.crs_id" => json!(crs),
        "plenora.geometry.crs_definition" => {
            return json!(plenora_io::crs_projjson::projjson_di(crs)
                .unwrap()
                .to_string())
        }
        "epoch" | "plenora.geometry.native.geoparquet.epoch" => {
            json!(if different { 2024.5 } else { 2023.5 })
        }
        "edges" | "plenora.geometry.native.geoparquet.edges" => json!("planar"),
        "bbox" | "plenora.geometry.native.geoparquet.bbox" => {
            json!([-100_000_000, -100_000_000, 100_000_000, 100_000_000])
        }
        "covering" => json!({"bbox":{"xmin":[field.name()]}}),
        "geometry_types" => json!(["Point"]),
        "precision" => json!(0.01),
        "dimensions" | "plenora.geometry.dimensions" => json!("xy"),
        "encoding" | "plenora.geometry.encoding" => json!("wkb"),
        "types" | "plenora.geometry.types" => json!(ALL_TYPES),
        "types_declaration" | "plenora.geometry.types_declaration" => json!("mixed"),
        "axis_order" | "plenora.geometry.axis_order" => json!(field
            .metadata()
            .get("plenora.geometry.axis_order")
            .map_or("easting_northing", String::as_str)),
        "crs_type" | "plenora.geometry.crs_definition_format" => json!("projjson"),
        "plenora.geometry.crs_resolution" => json!("resolved"),
        "plenora.geometry.srid" => json!("100001"),
        "plenora.geometry.spatial_semantics" => json!("geometry"),
        "plenora.geometry.precision" => json!(if different { "float32" } else { "native" }),
        "ARROW:extension:name" => json!("geoarrow.wkb"),
        _ => json!(if different { "matrice-b" } else { "matrice-a" }),
    };
    if matches!(site, Site::Field) {
        json!(value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned))
    } else {
        value
    }
}
fn mark(batch: &RecordBatch, key: &str, site: Site, mode: usize, origin: usize) -> RecordBatch {
    if matches!(site, Site::SchemaGeo) {
        let mut columns = serde_json::Map::new();
        let mut primary = None;
        for field in batch.schema().fields().iter().filter(|f| e_geometria(f)) {
            let id = field.metadata().get("plenora.geometry.crs_id").unwrap();
            let mut column = json!({"encoding":"WKB","geometry_types":[],"crs":plenora_io::crs_projjson::projjson_di(id).unwrap()});
            let value = match key {
                "crs" | "encoding" | "geometry_types" => {
                    let value = column[key].clone();
                    if mode == 2 && origin > 0 {
                        conflict(value)
                    } else {
                        value
                    }
                }
                _ => sample(key, field, Site::Geo, mode == 2 && origin > 0),
            };
            if mode != 1 || origin == 0 {
                column[key] = value;
            } else {
                column.as_object_mut().unwrap().remove(key);
            }
            columns.insert(field.name().clone(), column);
            primary.get_or_insert_with(|| field.name().clone());
        }
        let mut md = batch.schema().metadata().clone();
        if let Some(primary) = primary {
            md.insert(
                "geo",
                json!({"version":"1.1.0","primary_column":primary,"columns":columns}).to_string(),
            );
        }
        return RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                batch.schema().fields().clone(),
                md,
            )),
            batch.columns().to_vec(),
        )
        .unwrap();
    }
    let fields: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| {
            let mut md = f.metadata().clone();
            if e_geometria(f) {
                let value = if key == "plenora.geometry.types" && mode == 0 {
                    input_types(batch, f)
                } else {
                    sample(key, f, site, mode == 2 && origin > 0)
                };
                if let Some(container) = site.container() {
                    let mut object = md
                        .get(container)
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .unwrap_or_else(|| json!({}));
                    if mode == 1 && origin > 0 {
                        object.as_object_mut().unwrap().remove(key);
                    } else {
                        object[key] = value;
                    }
                    md.insert(container, object.to_string());
                } else if mode == 1 && origin > 0 {
                    md.remove(key);
                } else {
                    md.insert(key, value.as_str().unwrap());
                    if key == "plenora.geometry.types" {
                        md.insert("plenora.geometry.types_declaration", "mixed");
                    }
                    if matches!(
                        key,
                        "plenora.geometry.crs_definition"
                            | "plenora.geometry.crs_definition_format"
                    ) {
                        let crs = md.get("plenora.geometry.crs_id").unwrap();
                        let definition = plenora_io::crs_projjson::projjson_di(crs)
                            .unwrap()
                            .to_string();
                        if mode != 2 || origin == 0 {
                            md.insert("plenora.geometry.crs_definition", definition);
                            md.insert("plenora.geometry.crs_definition_format", "projjson");
                        }
                    }
                }
            }
            f.as_ref().clone().with_metadata(md)
        })
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        batch.columns().to_vec(),
    )
    .unwrap()
}
fn execute(
    scheda: &Scheda,
    batches: &[RecordBatch],
    how: Option<&str>,
) -> Result<RecordBatch, PlenoraError> {
    let mut plan: Value = serde_json::from_str(&testo_piano(scheda)).unwrap();
    if let Some(how) = how {
        plan["steps"][0]["config"]["how"] = how.into();
    }
    let pipeline = Pipeline::from_json(&plan.to_string())?;
    let schemas: Vec<_> = scheda
        .esempio
        .ingressi
        .iter()
        .zip(batches)
        .map(|(s, b)| (s.nome.as_str(), b.schema()))
        .collect();
    let input = scheda
        .esempio
        .ingressi
        .iter()
        .zip(batches)
        .map(|(s, b)| (s.nome.clone(), b.clone()))
        .collect();
    Ok(pipeline.validate(&schemas)?.run(input)?.outputs.remove(0).1)
}
fn input_transport(
    batch: &RecordBatch,
    path: &Path,
    site: Site,
) -> Result<RecordBatch, PlenoraError> {
    if matches!(site, Site::SchemaGeo)
        && path.extension().is_some_and(|e| e == "parquet")
        && batch.schema().metadata().contains_key("geo")
    {
        // Footer della fixture scritto direttamente: il writer di produzione
        // rigenererebbe le sonde prima che raggiungano la pipeline.
        let raw = batch.schema().metadata().get("geo").cloned().unwrap();
        let props = parquet::file::properties::WriterProperties::builder()
            .set_key_value_metadata(Some(vec![parquet::file::metadata::KeyValue::new(
                "geo".into(),
                raw,
            )]))
            .build();
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(path)?,
            batch.schema(),
            Some(props),
        )
        .map_err(|_| PlenoraError::Internal("scrittura fixture Parquet".into()))?;
        writer
            .write(batch)
            .map_err(|_| PlenoraError::Internal("scrittura fixture Parquet".into()))?;
        writer
            .close()
            .map_err(|_| PlenoraError::Internal("chiusura fixture Parquet".into()))?;
    } else {
        transport(batch, path)?;
    }
    plenora_io::leggi_tabella(path, None, u64::MAX)
}
fn transport(batch: &RecordBatch, path: &Path) -> Result<RecordBatch, PlenoraError> {
    // Writer pubblici su file reali: la durabilita' atomica ha prove proprie.
    // Il reader Arrow e' indipendente dal codec di lettura di plenora-io.
    let file = std::fs::File::create(path)?;
    let extension = path.extension().unwrap().to_str().unwrap();
    match extension {
        "arrow" => plenora_io::ipc::scrivi(batch, file)?,
        "arrows" => plenora_io::ipc::scrivi_stream(batch, file)?,
        "parquet" => {
            plenora_io::parquet_io::scrivi(
                batch,
                file,
                plenora_io::CompressioneParquet::default(),
            )?;
        }
        _ => unreachable!(),
    }
    let file = std::fs::File::open(path)?;
    let (schema, batches) = match extension {
        "arrow" => {
            let reader = ArrowFileReader::try_new(file, None)?;
            (reader.schema(), reader.collect::<Result<Vec<_>, _>>()?)
        }
        "arrows" => {
            let reader = StreamReader::try_new(file, None)?;
            (reader.schema(), reader.collect::<Result<Vec<_>, _>>()?)
        }
        "parquet" => {
            let builder = ParquetRecordBatchReaderBuilder::try_new(file)
                .map_err(|_| PlenoraError::Schema("lettura Parquet pubblico".into()))?;
            let schema = builder.schema().clone();
            let reader = builder
                .build()
                .map_err(|_| PlenoraError::Schema("lettura Parquet pubblico".into()))?;
            (schema, reader.collect::<Result<Vec<_>, _>>()?)
        }
        _ => unreachable!(),
    };
    Ok(plenora_core::arrow::select::concat::concat_batches(
        &schema, &batches,
    )?)
}

#[allow(clippy::too_many_lines)] // Oracolo del documento completo.
fn footer(path: &Path, output: &RecordBatch) -> Result<(), String> {
    let reader = SerializedFileReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let raw = reader
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .and_then(|kv| kv.iter().find(|kv| kv.key == "geo"))
        .and_then(|kv| kv.value.as_deref());
    let fields: Vec<_> = output
        .schema()
        .fields()
        .iter()
        .filter(|f| e_geometria(f))
        .cloned()
        .collect();
    if fields.is_empty() {
        return if raw.is_none() {
            Ok(())
        } else {
            Err("footer geo senza geometria".into())
        };
    }
    let geo: Value =
        serde_json::from_str(raw.ok_or("footer geo assente")?).map_err(|e| e.to_string())?;
    if geo["version"] != "1.1.0" || geo["primary_column"] != fields[0].name().as_str() {
        return Err("intestazione footer".into());
    }
    for field in fields {
        let column = &geo["columns"][field.name()];
        if column["encoding"] != "WKB" {
            return Err("encoding footer".into());
        }
        let expected_crs = if let Some(id) = field.metadata().get("plenora.geometry.crs_id") {
            plenora_io::crs_projjson::projjson_di(id).map_err(|e| e.to_string())?
        } else {
            serde_json::from_str::<Value>(
                field
                    .metadata()
                    .get("plenora.geometry.crs_definition")
                    .ok_or("CRS assente")?,
            )
            .map_err(|e| e.to_string())?
        };
        if column["crs"] != expected_crs {
            return Err("CRS footer".into());
        }
        // Riepilogo indipendente: decoder del kernel e BoundingRect di geo,
        // senza richiamare il sommario WKB del writer GeoParquet.
        let array = output
            .column_by_name(field.name())
            .ok_or("colonna assente")?;
        let cells = array
            .as_any()
            .downcast_ref::<plenora_core::arrow::array::BinaryArray>()
            .ok_or("WKB non Binary")?;
        let mut bounds: Option<[f64; 4]> = None;
        let mut actual_types = BTreeSet::new();
        for bytes in cells.iter().flatten() {
            let geometry = plenora_kernels_geo::arrow_adapter::decode_geometry_cell(bytes)
                .map_err(|_| "decodifica indipendente WKB")?;
            let kind = match &geometry {
                Geometry::Point(_) => "Point",
                Geometry::LineString(_) => "LineString",
                Geometry::Polygon(_) => "Polygon",
                Geometry::MultiPoint(_) => "MultiPoint",
                Geometry::MultiLineString(_) => "MultiLineString",
                Geometry::MultiPolygon(_) => "MultiPolygon",
                Geometry::GeometryCollection(_) => "GeometryCollection",
                _ => return Err("tipo WKB inatteso".into()),
            };
            actual_types.insert(kind.to_owned());
            if let Some(rect) = geometry.bounding_rect() {
                let b = [rect.min().x, rect.min().y, rect.max().x, rect.max().y];
                bounds = Some(bounds.map_or(b, |old| {
                    [
                        old[0].min(b[0]),
                        old[1].min(b[1]),
                        old[2].max(b[2]),
                        old[3].max(b[3]),
                    ]
                }));
            }
        }
        if field
            .metadata()
            .get("plenora.geometry.dimensions")
            .is_some_and(|d| d == "xy")
        {
            if column.get("bbox").cloned() != bounds.map(|b| json!(b)) {
                return Err("bbox footer diverso dai dati".into());
            }
            let types = field.metadata().get("plenora.geometry.types");
            let expected_types: BTreeSet<String> = types.map_or_else(
                || {
                    if field
                        .metadata()
                        .get("plenora.geometry.types_declaration")
                        .is_some_and(|v| v != "unresolved")
                    {
                        BTreeSet::new()
                    } else {
                        actual_types
                    }
                },
                |types| {
                    types
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|t| {
                            match t {
                                "point" => "Point",
                                "linestring" => "LineString",
                                "polygon" => "Polygon",
                                "multipoint" => "MultiPoint",
                                "multilinestring" => "MultiLineString",
                                "multipolygon" => "MultiPolygon",
                                "geometrycollection" => "GeometryCollection",
                                _ => "unsupported",
                            }
                            .to_owned()
                        })
                        .collect()
                },
            );
            let actual: BTreeSet<_> = column["geometry_types"]
                .as_array()
                .ok_or("tipi footer assenti")?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect();
            if actual != expected_types {
                return Err("tipi footer diversi dalla dichiarazione".into());
            }
        }
        if column.get("covering").is_some()
            || column.get("orientation").is_some()
            || column.get("precision").is_some()
        {
            return Err("riepiloghi non rigenerati nel footer".into());
        }
        for key in ["epoch", "edges"] {
            let native = format!("plenora.geometry.native.geoparquet.{key}");
            let expected = field.metadata().get(&native).map(|v| {
                if key == "epoch" {
                    serde_json::from_str::<Value>(v).unwrap()
                } else {
                    json!(v)
                }
            });
            let expected = expected
                .or_else(|| read(&field, key, Site::Geo))
                .or_else(|| read(&field, key, Site::Extension));
            if column.get(key).cloned() != expected {
                return Err(format!("{key} footer"));
            }
        }
    }
    Ok(())
}

struct Audit {
    counts: [usize; 3],
    checked: [usize; 3],
    refusals: BTreeMap<String, usize>,
    divergences: Vec<String>,
}
#[allow(clippy::too_many_lines)] // Casi indipendenti, raccolti prima del fallimento.
fn run_part(operations: &'static [OperationDescriptor]) -> Audit {
    let classifications: BTreeMap<_, _> = include_str!("../riferimenti/operazioni_metadati.txt")
        .lines()
        .map(|line| line.split_once('\t').unwrap())
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let mut divergences = Vec::new();
    let mut counts = [0_usize; 3];
    let mut checked = [0_usize; 3];
    let mut refusals = BTreeMap::<String, usize>::new();
    for op in operations {
        assert_eq!(
            format!("{:?}", policy::operation_effect(op)),
            classifications[op.id]
        );
        let scheda = leggi_scheda(op).unwrap();
        let originals: Vec<_> = scheda
            .esempio
            .ingressi
            .iter()
            .map(|s| tabella_con_metadati(&s.colonne, &s.metadati).unwrap())
            .collect();
        let variants = if op.id == "table.join" {
            vec![
                (Some("inner"), 0),
                (Some("left"), 0),
                (Some("inner"), 1),
                (Some("left"), 1),
                (Some("inner"), 2),
                (Some("left"), 2),
            ]
        } else {
            vec![(None, 2)]
        };
        for (how, sides) in variants {
            let bases: Vec<_> = originals
                .iter()
                .enumerate()
                .map(|(i, b)| fixture(b, i, sides == 2 || sides == i))
                .collect();
            for (transport_index, ext) in ["arrow", "arrows", "parquet"].iter().enumerate() {
                let baseline = execute(&scheda, &bases, how)
                    .and_then(|b| transport(&b, &dir.path().join(format!("baseline.{ext}"))));
                for &(key, class) in policy::KEYS {
                    for site in sites(key) {
                        let merge = matches!(
                            op.id,
                            "table.concat" | "table.concat_by_name" | "table.union_distinct"
                        );
                        for mode in 0..if merge { 3 } else { 1 } {
                            let label = format!("{} / {key} / {site:?} / {ext} / {how:?} / lati={sides} / fusione={mode}",op.id);
                            let mut inputs = Vec::new();
                            let mut input_error = None;
                            for (i, b) in bases.iter().enumerate() {
                                let b = mark(b, key, site, mode, i);
                                match input_transport(
                                    &b,
                                    &dir.path().join(format!("in-{i}.{ext}")),
                                    site,
                                ) {
                                    Ok(b) => inputs.push(b),
                                    Err(e) => {
                                        input_error = Some(e);
                                        break;
                                    }
                                }
                            }
                            if let Some(error) = input_error {
                                if matches!(error, PlenoraError::Internal(_) | PlenoraError::Io(_))
                                {
                                    divergences
                                        .push(format!("{label}: errore interno/IO in ingresso"));
                                }
                                *refusals
                                    .entry(format!("{label} / ingresso / {:?}", error.category()))
                                    .or_default() += 1;
                                continue;
                            }
                            let must_reject = merge
                                && inputs.windows(2).any(|pair| {
                                    pair[0]
                                        .schema()
                                        .fields()
                                        .iter()
                                        .filter(|f| e_geometria(f))
                                        .any(|left| {
                                            pair[1].schema().field_with_name(left.name()).is_ok_and(
                                                |right| {
                                                    policy::merge(left.metadata(), right.metadata())
                                                        .is_err()
                                                },
                                            )
                                        })
                                });
                            let output = match execute(&scheda, &inputs, how) {
                                Ok(b) => b,
                                Err(e) => {
                                    if matches!(e, PlenoraError::Internal(_) | PlenoraError::Io(_))
                                    {
                                        divergences.push(format!("{label}: errore interno/IO"));
                                    }
                                    *refusals
                                        .entry(format!(
                                            "{label} / validate-run / {:?}",
                                            e.category()
                                        ))
                                        .or_default() += 1;
                                    continue;
                                }
                            };
                            if must_reject {
                                divergences.push(format!(
                                    "{label}: fusione accettata contro il rifiuto della politica"
                                ));
                            }
                            let path = dir.path().join(format!("out.{ext}"));
                            let before_write = output.clone();
                            let output = match transport(&output, &path) {
                                Ok(b) => b,
                                Err(e) => {
                                    divergences.push(format!(
                                        "{label}: uscita non rileggibile / {:?}",
                                        e.category()
                                    ));
                                    *refusals
                                        .entry(format!("{label} / uscita / {:?}", e.category()))
                                        .or_default() += 1;
                                    continue;
                                }
                            };
                            counts[transport_index] += 1;
                            let effect = policy::operation_effect(op);
                            if matches!(site, Site::SchemaGeo) {
                                if *ext != "parquet" {
                                    for source in &inputs {
                                        for f in source
                                            .schema()
                                            .fields()
                                            .iter()
                                            .filter(|f| e_geometria(f))
                                        {
                                            let expected = if effect == Effect::Unchanged
                                                && (inputs.len() == 1
                                                    || op.id == "table.assert_foreign_key")
                                                && output
                                                    .schema()
                                                    .fields()
                                                    .iter()
                                                    .any(|f| e_geometria(f))
                                            {
                                                read_schema(source, key, f.name())
                                            } else {
                                                None
                                            };
                                            let actual = read_schema(&output, key, f.name());
                                            checked[transport_index] += 1;
                                            if actual != expected {
                                                divergences.push(format!(
                                                    "{label}: geo di schema diverso"
                                                ));
                                            }
                                        }
                                    }
                                }
                                if *ext == "parquet" {
                                    if let Err(e) = footer(&path, &before_write) {
                                        divergences.push(format!("{label}: {e}"));
                                    }
                                    checked[transport_index] += 1;
                                }
                                continue;
                            }

                            if !output.schema().fields().iter().any(|f| e_geometria(f)) {
                                checked[transport_index] += 1;
                                if baseline.as_ref().is_ok_and(|b| {
                                    b.schema().fields().iter().any(|f| e_geometria(f))
                                }) {
                                    divergences.push(format!(
                                        "{label}: geometria eliminata solo nel caso marcato"
                                    ));
                                }
                            }
                            for f in output.schema().fields().iter().filter(|f| e_geometria(f)) {
                                let origin = f
                                    .metadata()
                                    .get(ORIGIN)
                                    .and_then(|s| s.parse::<usize>().ok())
                                    .or_else(|| {
                                        (class == Class::Crs
                                            && matches!(
                                                op.id,
                                                "geo.coverage_validate" | "geo.shared_paths"
                                            ))
                                        .then_some(0)
                                    });
                                let source = origin.and_then(|i| inputs.get(i)).and_then(|b| {
                                    b.schema().fields().iter().find(|f| e_geometria(f)).cloned()
                                });
                                let base_field = baseline.as_ref().ok().and_then(|b| {
                                    b.schema().field_with_name(f.name()).ok().cloned()
                                });
                                let inherited = source.as_ref().and_then(|f| read(f, key, site));
                                let regenerated =
                                    base_field.as_ref().and_then(|f| read(f, key, site));
                                let equal = inputs
                                    .iter()
                                    .filter_map(|b| {
                                        b.schema().fields().iter().find(|f| e_geometria(f)).cloned()
                                    })
                                    .map(|f| read(&f, key, site))
                                    .all(|v| v == inherited);
                                let preserve = source.is_some()
                                    && if site.container().is_some() {
                                        policy::keep_member(key, effect, equal)
                                    } else {
                                        policy::keep(key, effect, equal)
                                    };
                                let preserves_types = source.is_some()
                                    && effect != Effect::Merge
                                    && effect != Effect::Types;
                                let recoded = canonical(key)
                                    && matches!(
                                        effect,
                                        Effect::Values | Effect::Types | Effect::Crs
                                    )
                                    && !(class == Class::Crs
                                        && matches!(effect, Effect::Values | Effect::Types))
                                    && !(class == Class::Types && preserves_types);
                                let container_encoding = effect != Effect::Merge
                                    && key == "encoding"
                                    && site.container().is_some()
                                    && !(effect == Effect::Crs && matches!(site, Site::Geo));
                                let expected = match mapped_types(key, op.id, source.as_deref()) {
                                    TypeExpectation::Value(mapped) => mapped,
                                    TypeExpectation::Other => {
                                        if recoded {
                                            regenerated
                                        } else if preserve
                                            || container_encoding
                                            || (canonical(key)
                                                && class == Class::Types
                                                && preserves_types)
                                        {
                                            inherited
                                        } else if source.is_none()
                                            || canonical(key)
                                            || (effect == Effect::Crs
                                                && matches!(site, Site::Geo)
                                                && matches!(key, "crs" | "dimensions" | "encoding"))
                                        {
                                            regenerated
                                        } else {
                                            None
                                        }
                                    }
                                };
                                let actual = read(f, key, site);
                                checked[transport_index] += 1;
                                if actual != expected {
                                    divergences.push(format!("{label}: {effect:?} {class:?}, dichiarazione diversa (origine={origin:?})"));
                                }
                            }
                            if *ext == "parquet" {
                                if let Err(e) = footer(&path, &before_write) {
                                    divergences.push(format!("{label}: {e}"));
                                }
                            }
                        }
                    }
                }
            }
        }
        eprintln!("matrice: {}; uscite={counts:?}", op.id);
    }
    Audit {
        counts,
        checked,
        refusals,
        divergences,
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Raccolta completa prima del fallimento.
fn catalogo_chiavi_trasporti() {
    prepara_ambiente();
    let classifications: BTreeMap<_, _> = include_str!("../riferimenti/operazioni_metadati.txt")
        .lines()
        .map(|line| line.split_once('\t').unwrap())
        .collect();
    assert_eq!(
        classifications.keys().copied().collect::<BTreeSet<_>>(),
        CATALOG.iter().map(|op| op.id).collect(),
        "operazione senza classificazione"
    );
    let audits = std::thread::scope(|scope| {
        #[allow(clippy::needless_collect)] // Avvia tutti i worker prima dei join.
        let handles: Vec<_> = CATALOG
            .chunks(CATALOG.len().div_ceil(6))
            .map(|part| scope.spawn(move || run_part(part)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("worker della matrice"))
            .collect::<Vec<_>>()
    });
    let mut counts = [0_usize; 3];
    let mut checked = [0_usize; 3];
    let mut refusals = BTreeMap::<String, usize>::new();
    let mut divergences = Vec::new();
    for audit in audits {
        for i in 0..3 {
            counts[i] += audit.counts[i];
            checked[i] += audit.checked[i];
        }
        for (label, count) in audit.refusals {
            *refusals.entry(label).or_default() += count;
        }
        divergences.extend(audit.divergences);
    }
    divergences.sort();
    let mut report = format!("# Matrice metadati\n\nUscite rilette: {counts:?}. Confronti: {checked:?}.\n\n## Rifiuti espliciti\n\n");
    for (label, count) in refusals {
        writeln!(report, "- {label}: {count}").unwrap();
    }
    report.push_str("\n## Divergenze\n\n");
    for d in &divergences {
        writeln!(report, "- {d}").unwrap();
    }
    std::fs::create_dir_all(radice().join("target")).unwrap();
    let initial = radice().join("target/matrice-metadati-iniziale.md");
    if !initial.exists() {
        std::fs::write(&initial, &report).unwrap();
    }
    std::fs::write(radice().join("target/matrice-metadati.md"), report).unwrap();
    eprintln!(
        "E2E: {counts:?}; confronti {checked:?}; divergenze {}",
        divergences.len()
    );
    assert!(divergences.is_empty(), "vedere target/matrice-metadati.md");
}
