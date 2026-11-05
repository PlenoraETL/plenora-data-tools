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
                md.insert(ORIGIN, format!("{origin}:{}", f.name()));
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
fn without_primary_geometry(batch: &RecordBatch) -> bool {
    batch.schema().fields().iter().any(|f| e_geometria(f)) && primary_name(batch).is_none()
}

fn primary_name(batch: &RecordBatch) -> Option<String> {
    plenora_core::contract::arrow_schema::primary_geometry_position(&batch.schema())
        .unwrap()
        .map(|p| batch.schema().field(p).name().clone())
}

#[allow(clippy::too_many_lines)] // Fixture unitaria: colonne, CRS e primaria coerenti.
fn plural(
    batch: RecordBatch,
    count: usize,
    primary: Option<usize>,
    selected_primary: bool,
) -> RecordBatch {
    if !batch.schema().fields().iter().any(|f| e_geometria(f)) {
        return batch;
    }
    let mut fields: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut columns = batch.columns().to_vec();
    let source = fields.iter().find(|f| e_geometria(f)).unwrap();
    let origin = source
        .metadata()
        .get(ORIGIN)
        .unwrap()
        .split_once(':')
        .unwrap()
        .0
        .to_owned();
    let source_geographic = source
        .metadata()
        .get("plenora.geometry.crs_id")
        .is_some_and(|id| id == "EPSG:4326");
    let source_utm32 = source
        .metadata()
        .get("plenora.geometry.crs_id")
        .is_some_and(|id| id == "EPSG:32632");
    for extra in 1..count {
        let name = format!("__passante_{extra}");
        let crs = if extra == 2 {
            if source_utm32 {
                "EPSG:32633"
            } else {
                "EPSG:32632"
            }
        } else if source_geographic {
            "EPSG:3857"
        } else {
            "EPSG:4326"
        };
        let mut field = geometry_output_field(&name, crs).unwrap();
        let mut md = field.metadata().clone();
        md.insert(
            "plenora.geometry.axis_order",
            if crs == "EPSG:4326" {
                "lon_lat"
            } else {
                "easting_northing"
            },
        );
        md.insert(ORIGIN, format!("{origin}:{name}"));
        field = field.with_metadata(md);
        let mut bytes = vec![1, 1, 0, 0, 0];
        bytes.extend_from_slice(&12_f64.to_le_bytes());
        bytes.extend_from_slice(&45_f64.to_le_bytes());
        let values = vec![Some(bytes.as_slice()); batch.num_rows()];
        let array: ArrayRef = Arc::new(plenora_core::arrow::array::BinaryArray::from(values));
        if extra == 2 {
            fields.insert(0, field);
            columns.insert(0, array);
        } else {
            fields.push(field);
            columns.push(array);
        }
    }
    let positions: Vec<_> = fields
        .iter()
        .enumerate()
        .filter(|(_, f)| e_geometria(f))
        .map(|(i, _)| i)
        .collect();
    let original = positions
        .iter()
        .position(|&i| !fields[i].name().starts_with("__passante_"))
        .unwrap();
    let desired = if selected_primary {
        primary.unwrap()
    } else if primary == Some(original) {
        (original + 1) % count
    } else {
        original
    };
    fields.swap(positions[original], positions[desired]);
    columns.swap(positions[original], positions[desired]);
    let mut geometries = Vec::new();
    for (index, field) in fields.iter_mut().enumerate() {
        let mut md = field.metadata().clone();
        md.insert("plenora.field_id", index.to_string());
        *field = field.clone().with_metadata(md);
        if e_geometria(field) {
            geometries.push(index);
        }
    }
    let mut md = batch.schema().metadata().clone();
    md.insert("plenora.contract.version", "1");
    md.insert(
        "plenora.primary_geometry",
        primary.map_or_else(|| "none".into(), |p| geometries[p].to_string()),
    );
    let schema = Arc::new(Schema::new_with_metadata(fields, md));
    let contract = contract_from_arrow_schema(schema, resolve_crs).unwrap();
    RecordBatch::try_new(arrow_schema_from_contract(&contract).unwrap(), columns).unwrap()
}

fn catalog_effect(op: &OperationDescriptor) -> Effect {
    let declared = include_str!("../riferimenti/operazioni_metadati.txt")
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(id, _)| *id == op.id)
        .expect("classificazione dell'operazione")
        .1;
    match declared {
        "Unchanged" => Effect::Unchanged,
        "Rows" => Effect::Rows,
        "Values" => Effect::Values,
        "Types" => Effect::Types,
        "Crs" => Effect::Crs,
        "Merge" => Effect::Merge,
        _ => panic!("classificazione non riconosciuta"),
    }
}

pub fn expected_effects(
    op: &OperationDescriptor,
    inputs: &[RecordBatch],
    output: &RecordBatch,
) -> Vec<(usize, Effect)> {
    use plenora_core::catalog::SourceRowProvenance;
    // Oracolo dalla regola scritta: nessuna chiamata a column_effects.
    let effect = if op.id == "table.table_diff" {
        Effect::Merge
    } else {
        catalog_effect(op)
    };
    output
        .schema()
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| e_geometria(field))
        .map(|(position, field)| {
            let origin = field.metadata().get(ORIGIN).and_then(|s| s.split_once(':'));
            let source_name = origin.map_or_else(|| field.name().as_str(), |(_, name)| name);
            let inherited = inputs.iter().any(|input| {
                input
                    .schema()
                    .field_with_name(source_name)
                    .is_ok_and(e_geometria)
            });
            let column_effect = if op.family != Family::Geo {
                effect
            } else if !inherited
                || (matches!(
                    op.id,
                    "geo.coverage_validate" | "geo.shared_paths" | "geo.generate_grid"
                ) && !source_name.starts_with("__passante_"))
            {
                // Le nuove geometrie derivate ereditano soltanto C1;
                // tipi e rappresentazione sono quelli prodotti dal codec.
                Effect::Types
            } else if !source_name.starts_with("__passante_")
                && !matches!(
                    op.id,
                    "geo.from_coords" | "geo.from_wkt" | "geo.generate_grid"
                )
            {
                effect
            } else if op.source_row_provenance() == SourceRowProvenance::Preserved {
                Effect::Unchanged
            } else {
                Effect::Rows
            };
            (position, column_effect)
        })
        .collect()
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
                Some(json!([90_000_000, 90_000_000, 90_000_001, 90_000_001]))
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
    if op == "table.table_diff" {
        return TypeExpectation::Value(if key == "plenora.geometry.types" {
            None
        } else {
            source
                .metadata()
                .contains_key("plenora.geometry.types_declaration")
                .then(|| json!("unresolved"))
        });
    }
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
    let ordinal = field
        .name()
        .strip_prefix("__passante_")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let probe = match key {
        "epoch" | "plenora.geometry.native.geoparquet.epoch" => {
            Some(json!(2023.5 + f64::from(ordinal)))
        }
        "bbox" | "plenora.geometry.native.geoparquet.bbox" => {
            let bound = 100_000_000 + i64::from(ordinal);
            Some(json!([bound, bound, bound + 1, bound + 1]))
        }
        "precision" => Some(json!(0.01 * f64::from(ordinal + 1))),
        "orientation" => Some(json!(format!("orientamento-colonna-{ordinal}"))),
        key if key.starts_with("plenora.geometry.native.")
            && !matches!(policy::class(key), Class::Crs) =>
        {
            Some(json!(format!("matrice-colonna-{ordinal}")))
        }
        _ => None,
    };
    if let Some(value) = probe {
        return if matches!(site, Site::Field) {
            json!(value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned))
        } else {
            value
        };
    }
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
            json!([100_000_000, 100_000_000, 100_000_001, 100_000_001])
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
        let primary = primary_name(batch);
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
    if scheda.operazione.family == Family::Geo
        && !matches!(
            scheda.operazione.id,
            "geo.from_wkt" | "geo.from_coords" | "geo.generate_grid"
        )
    {
        for (side, batch) in batches.iter().enumerate() {
            if let Some(field) = batch
                .schema()
                .fields()
                .iter()
                .find(|f| e_geometria(f) && !f.name().starts_with("__passante_"))
            {
                let key = if batches.len() == 2 {
                    if side == 0 {
                        "left_geometry"
                    } else {
                        "right_geometry"
                    }
                } else {
                    "input_geometry"
                };
                plan["steps"][0]["config"][key] = field.name().clone().into();
            }
        }
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
    if geo["version"] != "1.1.0"
        || geo["primary_column"].as_str() != primary_name(output).as_deref()
    {
        return Err("intestazione footer".into());
    }
    let actual_names = geo["columns"]
        .as_object()
        .ok_or("colonne footer assenti")?
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_names = fields
        .iter()
        .map(|f| f.name().as_str())
        .collect::<BTreeSet<_>>();
    if actual_names != expected_names {
        return Err("insieme colonne footer diverso".into());
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
// Ogni confine ha un solo esito: None significa successo obbligatorio.
// Le regole dipendono dalla fixture, mai dall'errore ottenuto.
type Rejection = Option<plenora_core::ErrorCategory>;

fn operation_rejection(op: &str, count: usize) -> Rejection {
    use plenora_core::ErrorCategory::{Crs, InvalidPlan, Unsupported};
    match op {
        "table.assert_schema" => Some(InvalidPlan), // La fixture aggiunge colonne.
        "table.pivot" => Some(Unsupported),         // Mapping assente nella scheda.
        // La fixture mescola sempre geometrie e scalari fra i valori:
        // la regola GT rifiuta anche una sola geometria, prima dello schema dinamico.
        "table.transpose" if count > 0 => Some(Crs),
        _ => None,
    }
}

fn invalid_probe(key: &str, site: Site, mode: usize) -> Rejection {
    use plenora_core::ErrorCategory::{Crs, Schema, Unsupported};
    if matches!(site, Site::SchemaGeo) || mode == 0 {
        return None;
    }
    // Un asse canonico mancante non e' ricostruibile; gli altri campi
    // omessi hanno default, oppure sono confrontati solo alla fusione.
    if key == "plenora.geometry.axis_order" {
        return Some(Crs);
    }
    if mode != 2 {
        return None;
    }
    match (key, site) {
        (
            "crs"
            | "plenora.geometry.crs_id"
            | "plenora.geometry.crs_resolution"
            | "plenora.geometry.crs_definition"
            | "plenora.geometry.crs_definition_format",
            _,
        ) => Some(Crs),
        (
            "plenora.geometry.dimensions"
            | "plenora.geometry.encoding"
            | "plenora.geometry.spatial_semantics"
            | "plenora.geometry.types"
            | "plenora.geometry.types_declaration",
            _,
        )
        | ("dimensions", Site::Geo) => Some(Schema),
        ("crs_type", Site::Extension)
        | ("edges", Site::Extension | Site::Geo)
        | ("encoding", Site::Geo)
        | ("plenora.geometry.native.geoparquet.edges", _) => Some(Unsupported),
        _ => None,
    }
}

fn input_rejection(key: &str, site: Site, mode: usize, ext: &str, no_primary: bool) -> Rejection {
    use plenora_core::ErrorCategory::{Crs, InvalidPlan, Schema, Unsupported};
    if ext != "parquet" {
        return None; // IPC trasporta le dichiarazioni senza interpretarle.
    }
    if no_primary {
        return Some(Unsupported);
    }
    if matches!(site, Site::SchemaGeo) {
        return match key {
            "orientation" => Some(InvalidPlan), // Sentinella fuori vocabolario.
            "encoding" | "geometry_types" if mode != 0 => Some(InvalidPlan),
            "crs" if mode != 0 => Some(Crs),
            "edges" if mode == 2 => Some(Unsupported),
            "encoding" | "geometry_types" | "crs" | "epoch" | "edges" | "bbox" | "covering" => None,
            _ => Some(Unsupported), // Membri fuori dal formato GeoParquet 1.1.
        };
    }
    if key == "ARROW:extension:name" && mode != 0 {
        return Some(Schema); // Contratto e dichiarazione Arrow discordanti.
    }
    invalid_probe(key, site, mode)
}

fn merge_rejection(key: &str, site: Site, mode: usize, ext: &str) -> Rejection {
    use plenora_core::ErrorCategory::{Crs, Schema};
    if mode == 0 || (matches!(site, Site::SchemaGeo) && ext != "parquet") {
        return None;
    }
    if let Some(category) = invalid_probe(key, site, mode) {
        return Some(category);
    }
    match (key, site) {
        (
            "ARROW:extension:name"
            | "epoch"
            | "edges"
            | "plenora.geometry.native.geoparquet.epoch"
            | "plenora.geometry.native.geoparquet.edges",
            _,
        )
        | ("axis_order", Site::Field | Site::Geo | Site::Extension)
        | ("crs_type", Site::Field) => Some(Schema),
        (
            "plenora.geometry.crs_definition"
            | "plenora.geometry.crs_definition_format"
            | "plenora.geometry.srid",
            _,
        ) if mode == 2 || key != "plenora.geometry.srid" => Some(Crs),
        // Tipi mancanti o diversi ma validi degradano a unresolved.
        _ => None,
    }
}

fn check_outcome<T>(
    result: &Result<T, PlenoraError>,
    expected: Rejection,
    label: &str,
    failures: &mut Vec<String>,
) -> bool {
    let actual = result.as_ref().err().map(|e| (e.category(), e.phase()));
    let expected = expected.map(|c| (c, plenora_core::ErrorPhase::Validate));
    if actual != expected {
        failures.push(format!("{label}: atteso {expected:?}, ottenuto {actual:?}"));
        return false;
    }
    true
}

#[test]
fn errori_interni_non_sono_rifiuti_espliciti() {
    use plenora_core::ErrorCategory::{Crs, Schema};
    for count in [1, 2, 3] {
        assert_eq!(operation_rejection("table.transpose", count), Some(Crs));
    }
    let success = Ok(());
    let refusal = Err::<(), _>(PlenoraError::Schema("sonda".into()));
    let internal = Err::<(), _>(PlenoraError::Internal("sonda".into()));
    let wrong_phase = Err::<(), _>(
        PlenoraError::Schema("sonda".into()).with_phase(plenora_core::ErrorPhase::Read),
    );
    let mut failures = Vec::new();
    assert!(!check_outcome(
        &refusal,
        None,
        "successo obbligatorio",
        &mut failures
    ));
    assert!(!check_outcome(
        &success,
        Some(Schema),
        "rifiuto obbligatorio",
        &mut failures
    ));
    assert!(!check_outcome(
        &internal,
        Some(Schema),
        "categoria",
        &mut failures
    ));
    assert!(!check_outcome(
        &wrong_phase,
        Some(Schema),
        "fase",
        &mut failures
    ));
    for ext in ["arrow", "arrows", "parquet"] {
        assert_eq!(
            input_rejection("plenora.geometry.types", Site::Field, 1, ext, false),
            None
        );
        assert_eq!(
            merge_rejection("plenora.geometry.types", Site::Field, 1, ext),
            None
        );
    }
}

#[allow(clippy::too_many_lines)] // Casi indipendenti, raccolti prima del fallimento.
fn run_part(operations: &'static [OperationDescriptor], full: bool) -> Audit {
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
        let configurations: Vec<_> = if full {
            [1, 2, 3]
                .into_iter()
                .flat_map(|count| {
                    if count == 1 {
                        vec![(1, Some(0), true), (1, None, false)]
                    } else if op.family == Family::Geo {
                        vec![
                            (count, Some(0), true),
                            (count, Some(count - 1), true),
                            (count, Some(0), false),
                            (count, Some(count - 1), false),
                            (count, None, false),
                        ]
                    } else {
                        vec![
                            (count, None, false),
                            (count, Some(0), false),
                            (count, Some(count - 1), false),
                        ]
                    }
                })
                .collect()
        } else if op.family == Family::Geo {
            vec![(2, None, false), (2, Some(1), false), (2, Some(0), true)]
        } else {
            vec![(2, None, false), (2, Some(1), false)]
        };
        let configurations: Vec<_> = configurations
            .into_iter()
            .flat_map(|(count, primary, selected)| {
                let modes = if op.id == "geo.subdivide" {
                    vec![false, true]
                } else {
                    vec![false]
                };
                modes
                    .into_iter()
                    .map(move |renamed| (count, primary, selected, renamed))
            })
            .collect();
        for (count, primary, selected_primary, renamed) in configurations {
            let mut scheda = leggi_scheda(op).unwrap();
            if renamed {
                let mut config: Value = serde_json::from_str(scheda.esempio.config.get()).unwrap();
                config["output_column"] = json!("__derivata");
                scheda.esempio.config = serde_json::value::to_raw_value(&config).unwrap();
            }
            for &(how, sides) in &variants {
                let bases: Vec<_> = originals
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        plural(
                            fixture(b, i, sides == 2 || sides == i),
                            count,
                            primary,
                            selected_primary,
                        )
                    })
                    .collect();
                for (transport_index, ext) in ["arrow", "arrows", "parquet"].iter().enumerate() {
                    let baseline_run = execute(&scheda, &bases, how);
                    check_outcome(
                        &baseline_run,
                        operation_rejection(op.id, count),
                        &format!("{} / {ext}: baseline operazione", op.id),
                        &mut divergences,
                    );
                    let baseline = baseline_run.and_then(|b| {
                        let result = transport(&b, &dir.path().join(format!("baseline.{ext}")));
                        let expected = (*ext == "parquet" && without_primary_geometry(&b))
                            .then_some(plenora_core::ErrorCategory::Unsupported);
                        check_outcome(
                            &result,
                            expected,
                            &format!("{} / {ext}: baseline uscita", op.id),
                            &mut divergences,
                        );
                        result
                    });
                    for &(key, class) in policy::KEYS {
                        for site in sites(key) {
                            let merge = matches!(
                                op.id,
                                "table.concat" | "table.concat_by_name" | "table.union_distinct"
                            );
                            for mode in 0..if merge { 3 } else { 1 } {
                                let label = format!("{} / G={count} primaria={primary:?} rinomina={renamed} / {key} / {site:?} / {ext} / {how:?} / lati={sides} / fusione={mode}",op.id);
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
                                let input_result = input_error.map_or(Ok(()), Err);
                                let expected_input = input_rejection(
                                    key,
                                    site,
                                    mode,
                                    ext,
                                    bases.iter().any(without_primary_geometry),
                                );
                                let conforms = check_outcome(
                                    &input_result,
                                    expected_input,
                                    &format!("{label} / ingresso"),
                                    &mut divergences,
                                );
                                if let Err(error) = input_result {
                                    *refusals
                                        .entry(format!(
                                            "{label} / ingresso / {:?}",
                                            error.category()
                                        ))
                                        .or_default() += 1;
                                    continue;
                                }
                                if !conforms {
                                    continue;
                                }
                                let expected_run = operation_rejection(op.id, count)
                                    .or_else(|| merge_rejection(key, site, mode, ext));
                                let result = execute(&scheda, &inputs, how);
                                let conforms = check_outcome(
                                    &result,
                                    expected_run,
                                    &format!("{label} / validate-run"),
                                    &mut divergences,
                                );
                                let output = match result {
                                    Ok(b) => b,
                                    Err(e) => {
                                        *refusals
                                            .entry(format!(
                                                "{label} / validate-run / {:?}",
                                                e.category()
                                            ))
                                            .or_default() += 1;
                                        continue;
                                    }
                                };
                                if matches!(op.id, "geo.coverage_validate" | "geo.shared_paths") {
                                    let expected = primary_name(&inputs[0])
                                        .filter(|name| !name.starts_with("__passante_"))
                                        .map(|_| "geometry".to_owned());
                                    checked[transport_index] += 1;
                                    if primary_name(&output) != expected {
                                        divergences.push(format!(
                                            "{label}: primaria derivata diversa dalla provenienza"
                                        ));
                                    }
                                }
                                if !conforms {
                                    continue;
                                }
                                // La sonda delle chiavi derivate deve riuscire e degradare entrambe le chiavi:
                                // controllare soltanto l'assenza dell'elenco non basta.
                                if merge && mode == 1 && key == "plenora.geometry.types" {
                                    for field in
                                        output.schema().fields().iter().filter(|f| e_geometria(f))
                                    {
                                        if field.metadata().contains_key("plenora.geometry.types")
                                            || field
                                                .metadata()
                                                .get("plenora.geometry.types_declaration")
                                                .map(String::as_str)
                                                != Some("unresolved")
                                        {
                                            divergences.push(format!(
                                                "{label}: tipi non degradati a unresolved"
                                            ));
                                        }
                                    }
                                }
                                let path = dir.path().join(format!("out.{ext}"));
                                let before_write = output.clone();
                                let result = transport(&output, &path);
                                let expected_output = (*ext == "parquet"
                                    && without_primary_geometry(&before_write))
                                .then_some(plenora_core::ErrorCategory::Unsupported);
                                let conforms = check_outcome(
                                    &result,
                                    expected_output,
                                    &format!("{label} / uscita"),
                                    &mut divergences,
                                );
                                let output = match result {
                                    Ok(b) if conforms => b,
                                    Ok(_) => continue,
                                    Err(e) => {
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
                                                    && primary_name(source) == primary_name(&output)
                                                    && source
                                                        .schema()
                                                        .fields()
                                                        .iter()
                                                        .filter(|f| e_geometria(f))
                                                        .map(|f| f.name())
                                                        .collect::<BTreeSet<_>>()
                                                        == output
                                                            .schema()
                                                            .fields()
                                                            .iter()
                                                            .filter(|f| e_geometria(f))
                                                            .map(|f| f.name())
                                                            .collect::<BTreeSet<_>>()
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
                                for f in output.schema().fields().iter().filter(|f| e_geometria(f))
                                {
                                    let origin =
                                        f.metadata().get(ORIGIN).and_then(|s| s.split_once(':'));
                                    if origin.is_some_and(|(_, name)| name != f.name())
                                        && !f.name().starts_with("__passante_")
                                        && matches!(
                                            op.id,
                                            "geo.from_coords"
                                                | "geo.from_wkt"
                                                | "geo.generate_grid"
                                                | "geo.coverage_validate"
                                                | "geo.shared_paths"
                                        )
                                    {
                                        divergences.push(format!(
                                            "{label}: colonna nuova con provenienza clonata"
                                        ));
                                    }
                                    let source = origin
                                        .and_then(|(i, name)| {
                                            i.parse::<usize>()
                                                .ok()
                                                .and_then(|i| inputs.get(i))
                                                .and_then(|b| {
                                                    b.schema().field_with_name(name).ok().cloned()
                                                })
                                        })
                                        .or_else(|| {
                                            if class == Class::Crs
                                                && matches!(
                                                    op.id,
                                                    "geo.coverage_validate" | "geo.shared_paths"
                                                )
                                            {
                                                inputs.first().and_then(|b| {
                                                    b.schema()
                                                        .fields()
                                                        .iter()
                                                        .find(|f| {
                                                            e_geometria(f)
                                                                && !f
                                                                    .name()
                                                                    .starts_with("__passante_")
                                                        })
                                                        .map(|f| f.as_ref().clone())
                                                })
                                            } else {
                                                None
                                            }
                                        });
                                    if f.name().starts_with("__passante_") && source.is_none() {
                                        divergences.push(format!("{label}: provenienza della passante assente o non risolvibile"));
                                        continue;
                                    }
                                    let index = output.schema().index_of(f.name()).unwrap();
                                    let effect = expected_effects(op, &inputs, &output)
                                        .into_iter()
                                        .find(|(p, _)| *p == index)
                                        .unwrap()
                                        .1;
                                    let base_field = baseline.as_ref().ok().and_then(|b| {
                                        b.schema().field_with_name(f.name()).ok().cloned()
                                    });
                                    let inherited =
                                        source.as_ref().and_then(|f| read(f, key, site));
                                    let regenerated =
                                        base_field.as_ref().and_then(|f| read(f, key, site));
                                    let equal = inputs
                                        .iter()
                                        .filter_map(|b| {
                                            b.schema().field_with_name(f.name()).ok().cloned()
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
                                    let expected =
                                        match if matches!(effect, Effect::Unchanged | Effect::Rows)
                                        {
                                            TypeExpectation::Other
                                        } else {
                                            mapped_types(key, op.id, source.as_ref())
                                        } {
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
                                                        && matches!(
                                                            key,
                                                            "crs" | "dimensions" | "encoding"
                                                        ))
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
    run_matrix(false);
}

#[test]
#[ignore = "matrice completa multigeometria"]
fn catalogo_chiavi_trasporti_completo() {
    run_matrix(true);
}

#[allow(clippy::too_many_lines)] // Raccolta completa prima del fallimento.
fn run_matrix(full: bool) {
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
            .map(|part| scope.spawn(move || run_part(part, full)))
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
    let mut report = format!("# Matrice metadati\n\nUscite rilette: {counts:?}. Confronti: {checked:?}.\n\n## Copertura\n\nPolitica verificata per colonna, con provenienza ingresso:nome, primaria risolta e bbox separati nel footer. I casi senza primaria rifiutati da GeoParquet sono conteggiati come rifiuti attesi. Le geometrie fra i valori di melt e pivot sono enumerate, con le altre operazioni del catalogo, dalla prova catalogo_geometria_in_ogni_ruolo_non_legge_wkb_come_testo in operazioni_doc.rs. Ogni scenario richiede un unico esito, con categoria e fase esatte, anche nella baseline e nei trasporti.\n\n## Rifiuti espliciti\n\n");
    writeln!(
        report,
        "Totale rifiuti: {}.\n",
        refusals.values().sum::<usize>()
    )
    .unwrap();
    for (label, count) in refusals {
        writeln!(report, "- {label}: {count}").unwrap();
    }
    write!(
        report,
        "\n## Divergenze\n\nTotale: {}.\n\n",
        divergences.len()
    )
    .unwrap();
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

#[test]
fn integrazione_sonde_tabellari() {
    prepara_ambiente();
    let index = CATALOG
        .iter()
        .position(|op| op.id == "table.table_diff")
        .unwrap();
    let audit = run_part(&CATALOG[index..=index], false);
    assert!(audit.divergences.is_empty(), "{:?}", audit.divergences);
}

#[test]
fn operandi_secondari_non_ereditano_bbox_e_passanti_seguono_righe() {
    prepara_ambiente();
    let dir = tempfile::tempdir().unwrap();
    for op in CATALOG.iter().filter(|op| {
        op.family == Family::Geo && matches!(catalog_effect(op), Effect::Values | Effect::Types)
    }) {
        if matches!(
            op.id,
            "geo.from_coords" | "geo.from_wkt" | "geo.generate_grid"
        ) {
            continue;
        }
        let scheda = leggi_scheda(op).unwrap();
        let inputs: Vec<_> = scheda
            .esempio
            .ingressi
            .iter()
            .enumerate()
            .map(|(side, spec)| {
                let original = tabella_con_metadati(&spec.colonne, &spec.metadati).unwrap();
                let batch = plural(fixture(&original, side, true), 2, Some(1), false);
                let batch = mark(
                    &batch,
                    "plenora.geometry.native.geoparquet.bbox",
                    Site::Field,
                    0,
                    side,
                );
                mark(&batch, "bbox", Site::Geo, 0, side)
            })
            .collect();
        let output = execute(&scheda, &inputs, None).unwrap_or_else(|e| panic!("{}: {e}", op.id));
        let output = transport(&output, &dir.path().join("probe.arrow")).unwrap();
        let effects = expected_effects(op, &inputs, &output);
        for (position, effect) in effects {
            let schema = output.schema();
            let field = schema.field(position);
            if matches!(effect, Effect::Values | Effect::Types) {
                assert!(
                    !field
                        .metadata()
                        .contains_key("plenora.geometry.native.geoparquet.bbox"),
                    "{}",
                    op.id
                );
                assert!(read(field, "bbox", Site::Geo).is_none(), "{}", op.id);
            } else if let Ok(source) = inputs[0].schema().field_with_name(field.name()) {
                let mut expected = source.metadata().clone();
                policy::apply(&mut expected, effect);
                expected.remove("plenora.field_id");
                let mut actual = field.metadata().clone();
                actual.remove("plenora.field_id");
                assert_eq!(actual, expected, "{}", op.id);
            }
        }
    }
}

fn same_geometry_semantics(left: &RecordBatch, right: &RecordBatch) -> bool {
    let left_schema = left.schema();
    let right_schema = right.schema();
    let a: Vec<_> = left_schema
        .fields()
        .iter()
        .filter(|f| e_geometria(f))
        .collect();
    let b: Vec<_> = right_schema
        .fields()
        .iter()
        .filter(|f| e_geometria(f))
        .collect();
    let lc = contract_from_arrow_schema(left_schema.clone(), resolve_crs).unwrap();
    let rc = contract_from_arrow_schema(right_schema.clone(), resolve_crs).unwrap();
    let references = lc.geometries.iter().zip(&rc.geometries).all(|(l, r)| {
        match (l.crs.as_resolved(), r.crs.as_resolved()) {
            (Some(l), Some(r)) => l.semantically_equals(r),
            (None, None) => true,
            _ => false,
        }
    });
    references
        && a.len() == b.len()
        && primary_name(left) == primary_name(right)
        && a.iter().zip(b).all(|(x, y)| {
            x.name() == y.name()
                && (!x.metadata().contains_key("plenora.geometry.crs_definition")
                    || y.metadata().contains_key("plenora.geometry.crs_definition"))
                && x.metadata()
                    .keys()
                    .chain(y.metadata().keys())
                    .filter(|key| canonical(key) || key.as_str() == "plenora.field_id")
                    // La rappresentazione CRS puo' cambiare in GeoParquet;
                    // sopra si confronta il riferimento risolto per intero.
                    .filter(|key| {
                        !matches!(
                            key.as_str(),
                            "plenora.geometry.crs_definition"
                                | "plenora.geometry.crs_definition_format"
                        )
                    })
                    .all(|key| x.metadata().get(key) == y.metadata().get(key))
        })
}

// GeoParquet dichiara exact i tipi osservati quando il campo non ne dichiara.
// Oracolo sui valori originali, senza passare per writer o reader.
fn parquet_type_completion(batch: &RecordBatch) -> RecordBatch {
    let fields: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| {
            let mut field = field.as_ref().clone();
            if e_geometria(&field)
                && matches!(
                    field
                        .metadata()
                        .get("plenora.geometry.dimensions")
                        .map(String::as_str),
                    Some("xy" | "xyz")
                )
                && plenora_core::contract::arrow_metadata::canonical_geometry_types(&field)
                    .unwrap()
                    .is_none_or(|t| {
                        t.declaration() == plenora_core::contract::TypesDeclaration::Unresolved
                    })
            {
                let types = input_types(batch, &field);
                let types = types.as_str().unwrap();
                let mut md = field.metadata().clone();
                if types.is_empty() {
                    md.insert("plenora.geometry.types_declaration", "unresolved");
                } else {
                    md.insert("plenora.geometry.types", types);
                    md.insert("plenora.geometry.types_declaration", "exact");
                }
                field = field.with_metadata(md);
            }
            field
        })
        .collect();
    plenora_core::batch_with_rows(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        batch.columns().to_vec(),
        batch.num_rows(),
    )
    .unwrap()
}

fn qualification_execute(
    spec: &Scheda,
    inputs: &[RecordBatch],
) -> Result<RecordBatch, PlenoraError> {
    if spec.operazione.id == "table.transpose" {
        // Schema dinamico: adapter pubblico, con tutte le geometrie fra i valori.
        let indices: Vec<_> = inputs[0]
            .schema()
            .fields()
            .iter()
            .enumerate()
            .filter(|(_, f)| e_geometria(f))
            .map(|(i, _)| i)
            .collect();
        let input = inputs[0].project(&indices).unwrap();
        return plenora_kernels_table::reshape::transpose(
            &input,
            &plenora_kernels_table::reshape::Transpose {
                id_column: None,
                output_columns: vec![],
                type_policy: None,
            },
            &plenora_kernels_table::Limits::default(),
        );
    }
    execute(spec, inputs, None)
}

/// Qualificazione dei trasporti senza sonde artificiali nei metadati.
#[allow(clippy::too_many_lines)]
fn qualification_part(operations: &'static [OperationDescriptor]) -> Audit {
    prepara_ambiente();
    let dir = tempfile::tempdir().unwrap();
    let mut cases = [0_usize; 3];
    let mut completed = [0_usize; 3];
    let mut refusals = BTreeMap::<String, usize>::new();
    let mut failures = BTreeSet::new();
    for op in operations {
        let _ = catalog_effect(op); // Ogni operazione nuova deve essere classificata.
        let mut spec = leggi_scheda(op).unwrap();
        if op.id == "table.assert_schema" {
            let mut config: Value = serde_json::from_str(spec.esempio.config.get()).unwrap();
            config["allow_extra"] = json!(true);
            config["ordered"] = json!(false);
            spec.esempio.config = serde_json::value::to_raw_value(&config).unwrap();
        }
        if op.id == "table.pivot" {
            let mut config: Value = serde_json::from_str(spec.esempio.config.get()).unwrap();
            config["mapping"] = json!({"gen":"gen", "feb":"feb"});
            spec.esempio.config = serde_json::value::to_raw_value(&config).unwrap();
        }
        let originals: Vec<_> = spec
            .esempio
            .ingressi
            .iter()
            .map(|s| tabella_con_metadati(&s.colonne, &s.metadati).unwrap())
            .collect();
        for count in [1, 2, 3] {
            let primaries = if count == 1 {
                vec![Some(0), None]
            } else {
                vec![Some(0), Some(count - 1), None]
            };
            for primary in primaries {
                let selections = if op.family == Family::Geo
                    && originals.len() == 2
                    && count > 1
                    && primary.is_some()
                {
                    vec![(false, false), (false, true), (true, false), (true, true)]
                } else {
                    vec![(false, false), (true, true)]
                };
                for (selected, right_selected) in selections {
                    if selected && primary.is_none() {
                        continue;
                    }
                    let mut nonempty = None;
                    for state in 0..3 {
                        let inputs: Vec<_> = originals
                            .iter()
                            .enumerate()
                            .map(|(i, b)| {
                                let batch = plural(
                                    fixture(b, i, true),
                                    count,
                                    primary,
                                    if i == 0 { selected } else { right_selected },
                                );
                                if state == 2 {
                                    return batch.slice(0, 0);
                                }
                                if state == 0 {
                                    return batch;
                                }
                                let columns = batch
                                    .columns()
                                    .iter()
                                    .zip(batch.schema().fields())
                                    .map(|(c, f)| {
                                        if e_geometria(f) {
                                            plenora_core::arrow::array::new_null_array(
                                                c.data_type(),
                                                c.len(),
                                            )
                                        } else {
                                            c.clone()
                                        }
                                    })
                                    .collect();
                                let fields: Vec<_> = batch
                                    .schema()
                                    .fields()
                                    .iter()
                                    .map(|f| f.as_ref().clone().with_nullable(true))
                                    .collect();
                                RecordBatch::try_new(
                                    Arc::new(Schema::new_with_metadata(
                                        fields,
                                        batch.schema().metadata().clone(),
                                    )),
                                    columns,
                                )
                                .unwrap()
                            })
                            .collect();
                        let expected = qualification_execute(&spec, &inputs);
                        let rejection = if (op.id == "geo.voronoi" && state != 0)
                            || (op.id == "table.assert_cardinality" && state == 2)
                        {
                            Some(plenora_core::ErrorCategory::InvalidPlan)
                        } else if op.id == "table.transpose" && count > 1 {
                            Some(plenora_core::ErrorCategory::Crs)
                        } else {
                            None
                        };
                        let mut outcome_failures = Vec::new();
                        if !check_outcome(
                            &expected,
                            rejection,
                            &format!("{} state={state} G={count} / operazione", op.id),
                            &mut outcome_failures,
                        ) {
                            failures.extend(outcome_failures);
                            failures.insert(format!(
                                "{} state={state} G={count}: atteso {rejection:?}, ottenuto {:?}",
                                op.id,
                                expected.as_ref().err().map(PlenoraError::category)
                            ));
                            continue;
                        }
                        if let Ok(output) = &expected {
                            if op.id != "table.transpose" {
                                if let Some(reference) = &nonempty {
                                    assert!(
                                        same_geometry_semantics(reference, output),
                                        "{}: schema geometrico diverso con null/vuoto",
                                        op.id
                                    );
                                } else {
                                    nonempty = Some(output.clone());
                                }
                            }
                        }
                        for (index, ext) in ["arrow", "arrows", "parquet"].iter().enumerate() {
                            let mut restored = Vec::new();
                            for (side, b) in inputs.iter().enumerate() {
                                let path = dir.path().join(format!("in-{side}.{ext}"));
                                let result = input_transport(b, &path, Site::Field);
                                let rejection = (*ext == "parquet" && without_primary_geometry(b))
                                    .then_some(plenora_core::ErrorCategory::Unsupported);
                                assert!(
                                    check_outcome(
                                        &result,
                                        rejection,
                                        &format!(
                                            "{} G={count} state={state} {ext} / ingresso {side}",
                                            op.id
                                        ),
                                        &mut outcome_failures
                                    ),
                                    "{outcome_failures:?}"
                                );
                                match result {
                                    Ok(b) => restored.push(b),
                                    Err(e) if *ext == "parquet" && without_primary_geometry(b) => {
                                        assert!(matches!(e, PlenoraError::Unsupported(_)));
                                        *refusals
                                            .entry(format!("{ext}: ingresso senza primaria"))
                                            .or_default() += 1;
                                        break;
                                    }
                                    Err(e) => {
                                        panic!("{} G={count} state={state} {ext}: {e}", op.id)
                                    }
                                }
                            }
                            cases[index] += 1;
                            if restored.len() != inputs.len() {
                                continue;
                            }
                            let output = qualification_execute(&spec, &restored);
                            assert!(
                                check_outcome(
                                    &output,
                                    rejection,
                                    &format!(
                                        "{} G={count} state={state} {ext} / operazione",
                                        op.id
                                    ),
                                    &mut outcome_failures
                                ),
                                "{outcome_failures:?}"
                            );
                            if let Some(category) = rejection {
                                assert_eq!(
                                    output.unwrap_err().category(),
                                    category,
                                    "{} {ext} rifiuto",
                                    op.id
                                );
                                *refusals
                                    .entry(format!("{ext}: operazione {category:?}"))
                                    .or_default() += 1;
                                continue;
                            }
                            let parquet_expected;
                            let expected = if *ext == "parquet" {
                                let completed: Vec<_> =
                                    inputs.iter().map(parquet_type_completion).collect();
                                parquet_expected =
                                    qualification_execute(&spec, &completed).unwrap();
                                &parquet_expected
                            } else {
                                expected.as_ref().unwrap()
                            };
                            let output = output
                                .unwrap_or_else(|e| panic!("{} {ext} state={state}: {e}", op.id));
                            assert!(
                                same_geometry_semantics(expected, &output),
                                "{} {ext}: semantica per colonna",
                                op.id
                            );
                            assert_eq!(
                                primary_name(&output),
                                primary_name(expected),
                                "{} primaria",
                                op.id
                            );
                            assert_eq!(
                                output.schema().fields().len(),
                                expected.num_columns(),
                                "{} colonne",
                                op.id
                            );
                            let config: Value =
                                serde_json::from_str(spec.esempio.config.get()).unwrap();
                            let generated = (op.id == "table.uuid_generator")
                                .then(|| config["output_column"].as_str().unwrap_or("uuid"));
                            for (i, field) in expected.schema().fields().iter().enumerate() {
                                if generated != Some(field.name().as_str()) {
                                    assert_eq!(
                                        output.column(i),
                                        expected.column(i),
                                        "{} {ext} valori",
                                        op.id
                                    );
                                }
                            }
                            for field in
                                expected.schema().fields().iter().filter(|f| e_geometria(f))
                            {
                                let actual = output.schema().index_of(field.name()).unwrap();
                                let source = expected.schema().index_of(field.name()).unwrap();
                                assert_eq!(
                                    output.column(actual),
                                    expected.column(source),
                                    "{} {ext}: byte geometrici",
                                    op.id
                                );
                            }
                            let path = dir.path().join(format!("out.{ext}"));
                            let result = transport(&output, &path);
                            let rejection = (*ext == "parquet"
                                && without_primary_geometry(&output))
                            .then_some(plenora_core::ErrorCategory::Unsupported);
                            assert!(
                                check_outcome(
                                    &result,
                                    rejection,
                                    &format!("{} G={count} state={state} {ext} / uscita", op.id),
                                    &mut outcome_failures
                                ),
                                "{outcome_failures:?}"
                            );
                            match result {
                                Ok(read) => {
                                    completed[index] += 1;
                                    assert!(
                                        same_geometry_semantics(&output, &read),
                                        "{} {ext}: semantica riletta",
                                        op.id
                                    );
                                    assert_eq!(
                                        primary_name(&read),
                                        primary_name(&output),
                                        "{} primaria riletta",
                                        op.id
                                    );
                                    assert_eq!(
                                        read.columns(),
                                        output.columns(),
                                        "{} byte riletti",
                                        op.id
                                    );
                                    if *ext == "parquet" {
                                        footer(&path, &output).unwrap();
                                    }
                                }
                                Err(e)
                                    if *ext == "parquet" && without_primary_geometry(&output) =>
                                {
                                    assert!(matches!(e, PlenoraError::Unsupported(_)));
                                    *refusals
                                        .entry(format!("{ext}: uscita senza primaria"))
                                        .or_default() += 1;
                                }
                                Err(e) => panic!("{} {ext}: {e}", op.id),
                            }
                        }
                    }
                }
            }
        }
    }
    Audit {
        counts: cases,
        checked: completed,
        refusals,
        divergences: failures.into_iter().collect(),
    }
}

#[test]
fn qualification_multigeometria_catalogo() {
    let audits = std::thread::scope(|scope| {
        #[allow(clippy::needless_collect)] // Avvio dei sei worker prima dei join.
        let handles: Vec<_> = CATALOG
            .chunks(CATALOG.len().div_ceil(6))
            .map(|part| scope.spawn(move || qualification_part(part)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut counts = [0_usize; 3];
    let mut failures = BTreeSet::new();
    let mut completed = [0_usize; 3];
    let mut refusals = BTreeMap::<String, usize>::new();
    for audit in audits {
        for i in 0..3 {
            counts[i] += audit.counts[i];
            completed[i] += audit.checked[i];
        }
        failures.extend(audit.divergences);
        for (key, count) in audit.refusals {
            *refusals.entry(key).or_default() += count;
        }
    }
    let report = format!(
        "Operazioni: {}. Tentativi IPC file/stream/GeoParquet: {counts:?}. Catene rilette: {completed:?}. Rifiuti attesi: {refusals:#?}. Divergenze: {failures:#?}\n",
        CATALOG.len()
    );
    std::fs::write(radice().join("target/qualificazione-mg5.txt"), &report).unwrap();
    eprintln!("{report}");
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn guida_geometrie_multiple() {
    let text = leggi(&radice().join("docs/geometrie-multiple.md"));
    let dir = tempfile::tempdir().unwrap();
    let mut examples = 0;
    for block in text.split("```json\n").skip(1) {
        let value: Value = serde_json::from_str(block.split_once("\n```").unwrap().0).unwrap();
        let specs: Vec<TabellaSpec> = serde_json::from_value(value["ingressi"].clone()).unwrap();
        let mut input = tabella_con_metadati(&specs[0].colonne, &specs[0].metadati).unwrap();
        let fields: Vec<_> = input
            .schema()
            .fields()
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut md = f.metadata().clone();
                md.insert("plenora.field_id", i.to_string());
                f.as_ref().clone().with_metadata(md)
            })
            .collect();
        input = RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields,
                input.schema().metadata().clone(),
            )),
            input.columns().to_vec(),
        )
        .unwrap();
        let mut contract = contract_from_arrow_schema(input.schema(), resolve_crs).unwrap();
        contract.active_geometry = Some(contract.field_ids[value["primaria"].as_str().unwrap()]);
        input = RecordBatch::try_new(
            arrow_schema_from_contract(&contract).unwrap(),
            input.columns().to_vec(),
        )
        .unwrap();
        let plan = Pipeline::from_json(&value["piano"].to_string()).unwrap();
        let result = plan
            .validate(&[("t", input.schema())])
            .unwrap()
            .run(vec![("t".into(), input)])
            .unwrap()
            .outputs
            .remove(0)
            .1;
        let expected = &value["attesa"];
        assert_eq!(
            primary_name(&result).as_deref(),
            expected["primaria"].as_str()
        );
        assert_eq!(
            result
                .schema()
                .fields()
                .iter()
                .filter(|f| e_geometria(f))
                .count(),
            usize::try_from(expected["geometrie"].as_u64().unwrap()).unwrap()
        );
        if let Some(rows) = expected["righe"].as_u64() {
            assert_eq!(result.num_rows(), usize::try_from(rows).unwrap());
        }
        if let Some(column) = expected["colonna"].as_str() {
            let array = result
                .column(result.schema().index_of(column).unwrap())
                .as_any()
                .downcast_ref::<plenora_core::arrow::array::BooleanArray>()
                .unwrap();
            for (row, v) in expected["valori"].as_array().unwrap().iter().enumerate() {
                assert_eq!(array.value(row), v.as_bool().unwrap());
            }
        }
        for ext in ["arrow", "arrows", "parquet"] {
            let path = dir.path().join(format!("guide.{ext}"));
            if ext == "parquet" && without_primary_geometry(&result) {
                assert!(matches!(
                    transport(&result, &path),
                    Err(PlenoraError::Unsupported(_))
                ));
            } else {
                let read = transport(&result, &path).unwrap();
                assert_eq!(read.columns(), result.columns());
                assert_eq!(primary_name(&read), primary_name(&result));
            }
        }
        examples += 1;
    }
    assert_eq!(examples, 3);
}

#[test]
fn oracolo_plurale_rifiuta_perdita_scambio_e_primaria() {
    let spec = leggi_scheda(find_operation("geo.area").unwrap()).unwrap();
    let input = &spec.esempio.ingressi[0];
    let original = tabella_con_metadati(&input.colonne, &input.metadati).unwrap();
    let batch = plural(fixture(&original, 0, true), 3, Some(0), true);
    assert!(same_geometry_semantics(&batch, &batch));
    // Entrambe le chiavi dei tipi devono partecipare anche quando le celle
    // e tutte le altre dichiarazioni restano identiche.
    let with_types = |declaration: &str, types: Option<&str>| {
        let fields: Vec<_> = batch
            .schema()
            .fields()
            .iter()
            .map(|f| {
                let mut md = f.metadata().clone();
                if e_geometria(f) {
                    md.insert("plenora.geometry.types_declaration", declaration);
                    md.remove("plenora.geometry.types");
                    if let Some(types) = types {
                        md.insert("plenora.geometry.types", types);
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
    };
    let declared = with_types("mixed", Some(ALL_TYPES));
    assert!(!same_geometry_semantics(
        &declared,
        &with_types("mixed", None)
    ));
    assert!(!same_geometry_semantics(
        &declared,
        &with_types("exact", Some(ALL_TYPES))
    ));
    let geometries: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| e_geometria(f))
        .map(|(i, _)| i)
        .collect();
    let keep: Vec<_> = (0..batch.num_columns())
        .filter(|i| *i != geometries[2])
        .collect();
    assert!(!same_geometry_semantics(
        &batch,
        &batch.project(&keep).unwrap()
    ));
    let mut fields = batch.schema().fields().to_vec();
    for (index, source) in [
        (geometries[1], geometries[2]),
        (geometries[2], geometries[1]),
    ] {
        let mut md = batch.schema().field(source).metadata().clone();
        md.insert(
            "plenora.field_id",
            batch
                .schema()
                .field(index)
                .metadata()
                .get("plenora.field_id")
                .unwrap()
                .clone(),
        );
        fields[index] = Arc::new(fields[index].as_ref().clone().with_metadata(md));
    }
    let swapped = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        batch.columns().to_vec(),
    )
    .unwrap();
    assert!(!same_geometry_semantics(&batch, &swapped));
    let mut md = batch.schema().metadata().clone();
    md.insert("plenora.primary_geometry", "none");
    let changed = RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            batch.schema().fields().clone(),
            md,
        )),
        batch.columns().to_vec(),
    )
    .unwrap();
    assert!(!same_geometry_semantics(&batch, &changed));
}

#[test]
fn qualificazione_tipi_geoparquet() {
    prepara_ambiente();
    let i = CATALOG
        .iter()
        .position(|op| op.id == "geo.from_coords")
        .unwrap();
    let audit = qualification_part(&CATALOG[i..=i]);
    assert!(audit.divergences.is_empty());
}
