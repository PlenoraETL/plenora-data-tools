//! Schemi attesi delle fixture geo, dalle forme dichiarate nelle schede.
//! Nessuna chiamata all'analisi o ai kernel dell'operazione sotto prova.
use plenora_core::arrow::array::RecordBatch;
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use serde_json::Value;
use std::sync::Arc;

pub fn verifica_schema_geo_indipendente(
    op: &str,
    output: &RecordBatch,
    input: &RecordBatch,
    config: &Value,
) {
    assert_eq!(
        output.schema(),
        schema_geo_indipendente(op, input, config, None),
        "schema indipendente {op}"
    );
}

#[allow(clippy::too_many_lines)]
pub fn schema_geo_indipendente(
    op: &str,
    input: &RecordBatch,
    config: &Value,
    crs_piano: Option<&str>,
) -> SchemaRef {
    let base =
        plenora_core::contract::arrow_schema::arrow_schema_from_contract(&super::contratto(input))
            .expect("schema fixture canonico");
    let mut fields: Vec<Field> = base.fields().iter().map(|f| f.as_ref().clone()).collect();
    let mut metadata = base.metadata().clone();
    let short = op.strip_prefix("geo.").expect("geo");
    let out = config
        .get("output_column")
        .and_then(Value::as_str)
        .unwrap_or(short);
    let pos = fields.iter().position(|f| f.name() == "geometry");
    let nullable = pos.is_none_or(|p| fields[p].is_nullable());
    let mut types = None;
    let scalar = match op {
        "geo.area"
        | "geo.length"
        | "geo.perimeter"
        | "geo.geodesic_line_length"
        | "geo.geodesic_area"
        | "geo.haversine_distance"
        | "geo.geodesic_distance"
        | "geo.bearing"
        | "geo.distance"
        | "geo.hausdorff_distance"
        | "geo.frechet_distance" => Some((out, DataType::Float64, true)),
        "geo.line_locate_point" => Some((
            config
                .get("output_column")
                .and_then(Value::as_str)
                .unwrap_or("fraction"),
            DataType::Float64,
            true,
        )),
        "geo.vertex_count" => Some((out, DataType::UInt64, nullable)),
        "geo.to_wkt" => Some((
            config
                .get("output_column")
                .and_then(Value::as_str)
                .unwrap_or("wkt"),
            DataType::Utf8,
            nullable,
        )),
        "geo.within" => Some((out, DataType::Boolean, nullable)),
        "geo.count_points_in_polygons" => Some((
            config
                .get("output_column")
                .and_then(Value::as_str)
                .unwrap_or("count"),
            DataType::UInt64,
            nullable,
        )),
        "geo.cluster_dbscan" => Some((
            config
                .get("output_column")
                .and_then(Value::as_str)
                .unwrap_or("cluster_id"),
            DataType::UInt64,
            true,
        )),
        _ if short.starts_with("predicate_") => Some((out, DataType::Boolean, nullable)),
        _ => None,
    };
    if let Some((name, t, n)) = scalar {
        fields.push(Field::new(name, t, n));
    } else {
        match op {
            "geo.centroid" | "geo.point_on_surface" | "geo.line_interpolate_point" => {
                types = Some("point");
            }
            "geo.convex_hull" | "geo.concave_hull" | "geo.voronoi" => types = Some("polygon"),
            "geo.envelope" => types = Some("point,linestring,polygon"),
            "geo.boundary" => types = Some("multipoint,multilinestring,geometrycollection"),
            "geo.buffer"
            | "geo.clip"
            | "geo.intersection"
            | "geo.union"
            | "geo.difference"
            | "geo.symmetric_difference" => types = Some("multipolygon"),
            "geo.line_substring" => types = Some("point,linestring"),
            "geo.clean_topology" => types = Some("polygon,multipolygon"),
            "geo.make_valid" => types = Some("mixed"),
            "geo.simplify"
            | "geo.affine_transform"
            | "geo.translate"
            | "geo.scale"
            | "geo.rotate"
            | "geo.densify"
            | "geo.snap_to_grid"
            | "geo.snap" => {}
            "geo.bounds_extractor" => {
                for suffix in ["minx", "miny", "maxx", "maxy"] {
                    fields.push(Field::new(
                        format!("geometry_{suffix}"),
                        DataType::Float64,
                        true,
                    ));
                }
            }
            "geo.geometry_accessors" => {
                let prefix = config
                    .get("output_prefix")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                for (name, t) in [
                    ("geometry_type", DataType::Utf8),
                    ("num_geometries", DataType::UInt64),
                    ("num_interior_rings", DataType::UInt64),
                    ("start_point", DataType::Utf8),
                    ("end_point", DataType::Utf8),
                    ("is_closed", DataType::Boolean),
                ] {
                    if config.get("fields").is_none_or(|v| {
                        v.as_array()
                            .expect("fields")
                            .iter()
                            .any(|v| v.as_str() == Some(name))
                    }) {
                        fields.push(Field::new(
                            format!("{prefix}{name}"),
                            t,
                            nullable || matches!(name, "start_point" | "end_point"),
                        ));
                    }
                }
            }
            "geo.geometry_diagnostics" => {
                let p = pos.expect("geometry");
                fields.remove(p);
                for (i, (name, t)) in [
                    ("geometry_type", DataType::Utf8),
                    ("coordinate_count", DataType::UInt64),
                    ("is_empty", DataType::Boolean),
                    ("is_finite", DataType::Boolean),
                    ("is_valid", DataType::Boolean),
                    ("validity_reason", DataType::Utf8),
                    ("bounds_minx", DataType::Float64),
                    ("bounds_miny", DataType::Float64),
                    ("bounds_maxx", DataType::Float64),
                    ("bounds_maxy", DataType::Float64),
                ]
                .into_iter()
                .enumerate()
                {
                    fields.insert(p + i, Field::new(name, t, nullable || i >= 5));
                }
            }
            "geo.reproject" => {
                let crs = config["target_crs"].as_str().expect("target");
                let replacement = super::tabella(crs, &[]);
                let canonical = plenora_core::contract::arrow_schema::arrow_schema_from_contract(
                    &super::contratto(&replacement),
                )
                .expect("target canonico");
                fields[pos.expect("geometry")] = canonical.field(2).clone().with_nullable(nullable);
            }
            "geo.explode" | "geo.delaunay" | "geo.split" | "geo.subdivide" => {
                let p = pos.expect("geometry");
                if op != "geo.subdivide" {
                    fields[p] = fields[p].clone().with_nullable(false);
                }
                if let Some(name) = config.get("output_column").and_then(Value::as_str) {
                    fields[p] = fields[p].clone().with_name(name);
                }
                fields.push(Field::new("__parent_index", DataType::UInt64, false));
            }
            "geo.dissolve"
            | "geo.line_builder"
            | "geo.polygon_builder"
            | "geo.line_merge"
            | "geo.polygonize"
            | "geo.collect"
            | "geo.overlay" => {
                let geometry = fields[pos.expect("geometry")]
                    .clone()
                    .with_nullable(!matches!(
                        op,
                        "geo.line_merge" | "geo.polygonize" | "geo.overlay"
                    ));
                let old = fields;
                fields = vec![geometry];
                if op == "geo.polygonize" {
                    fields.push(Field::new("__class", DataType::Utf8, false));
                }
                if op == "geo.collect" {
                    for name in config["group_by"].as_array().expect("group_by") {
                        fields.push(
                            old.iter()
                                .find(|f| Some(f.name().as_str()) == name.as_str())
                                .expect("chiave")
                                .clone(),
                        );
                    }
                }
                if op == "geo.overlay" {
                    fields.push(Field::new("__left_index", DataType::UInt64, true));
                    fields.push(Field::new("__right_index", DataType::UInt64, true));
                }
            }
            "geo.sjoin" | "geo.nearest" => {
                fields[pos.expect("geometry")] =
                    fields[pos.expect("geometry")].clone().with_nullable(false);
                fields.push(Field::new("__right_index", DataType::UInt64, false));
                if op == "geo.nearest" {
                    fields.push(Field::new(
                        config
                            .get("distance_column")
                            .and_then(Value::as_str)
                            .unwrap_or("distance"),
                        DataType::Float64,
                        false,
                    ));
                }
            }
            "geo.coverage_validate" | "geo.shared_paths" => {
                let geometry = fields[pos.expect("geometry")].clone().with_nullable(false);
                fields = if op == "geo.coverage_validate" {
                    vec![
                        Field::new("issue_type", DataType::Utf8, false),
                        Field::new("index_a", DataType::UInt64, false),
                        Field::new("index_b", DataType::UInt64, false),
                        Field::new("area", DataType::Float64, false),
                    ]
                } else {
                    vec![
                        Field::new("index_a", DataType::UInt64, false),
                        Field::new("index_b", DataType::UInt64, false),
                        Field::new("shared_length", DataType::Float64, false),
                    ]
                };
                fields.push(geometry);
            }
            "geo.generate_grid" | "geo.from_coords" | "geo.from_wkt" => {
                let crs = crs_piano.expect("CRS di piano per produttore");
                let replacement = super::tabella(crs, &[]);
                let canonical = plenora_core::contract::arrow_schema::arrow_schema_from_contract(
                    &super::contratto(&replacement),
                )
                .expect("CRS canonico");
                let geometry = canonical
                    .field(2)
                    .clone()
                    .with_nullable(op == "geo.from_wkt");
                if op == "geo.generate_grid" {
                    fields = vec![
                        geometry,
                        Field::new("cell_i", DataType::UInt64, false),
                        Field::new("cell_j", DataType::UInt64, false),
                    ];
                    types = Some("polygon");
                    if config.get("include_centroid").and_then(Value::as_bool) == Some(true) {
                        fields.push(Field::new("centroid_x", DataType::Float64, false));
                        fields.push(Field::new("centroid_y", DataType::Float64, false));
                    }
                } else {
                    fields.push(geometry);
                    if op == "geo.from_wkt" {
                        types=Some("mixed:point,linestring,polygon,multipoint,multilinestring,multipolygon,geometrycollection");
                    }
                }
            }
            _ => panic!("schema indipendente non dichiarato: {op}"),
        }
    }
    types = match op {
        "geo.dissolve" => Some("multipolygon"),
        "geo.overlay" => Some("polygon,multipolygon"),
        "geo.delaunay" => Some("polygon"),
        "geo.line_merge" => Some("linestring"),
        "geo.polygonize" | "geo.split" => Some("linestring,polygon"),
        _ => types,
    };
    if op == "geo.reproject" {
        let p = fields
            .iter()
            .position(|f| f.name() == "geometry")
            .expect("geometry");
        let mut m = fields[p].metadata().clone();
        let mut legacy: Value = serde_json::from_str(m.get("geo").expect("geo")).expect("geo json");
        legacy["encoding"] = Value::String("wkb".to_owned());
        m.insert("geo", serde_json::to_string(&legacy).expect("geo json"));
        fields[p] = fields[p].clone().with_metadata(m);
    }
    if let Some(types) = types {
        let p = fields
            .iter()
            .position(|f| f.name() == "geometry")
            .expect("geometry");
        let mut m = fields[p].metadata().clone();
        m.insert(
            "plenora.geometry.types_declaration",
            if types.starts_with("mixed") {
                "mixed"
            } else {
                "exact"
            },
        );
        if types == "mixed" {
            m.remove("plenora.geometry.types");
        } else {
            m.insert(
                "plenora.geometry.types",
                types.strip_prefix("mixed:").unwrap_or(types),
            );
        }
        fields[p] = fields[p].clone().with_metadata(m);
    }
    // Le fixture entrano senza identita' pubbliche: una sola uscita assegna
    // 0..N nell'ordine dei campi. Non leggiamo mai gli ID ottenuti.
    for (i, field) in fields.iter_mut().enumerate() {
        let mut m = field.metadata().clone();
        m.insert("plenora.field_id", i.to_string());
        *field = field.clone().with_metadata(m);
    }
    metadata.insert("plenora.contract.version", "1");
    metadata.remove("plenora.primary_geometry");
    Arc::new(Schema::new_with_metadata(fields, metadata))
}
