#![no_main]

//! `analyze_contract` dei kernel geo (Fase 2A-2b): config arbitrarie (base
//! valida fusa con JSON dal payload) su contratti sintetici con colonna
//! geometrica GeoArrow-WKB. Invarianti: mai panic; il contratto inferito, se
//! prodotto, ha schema senza nomi duplicati.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema};
use libfuzzer_sys::fuzz_target;
use plenora_core::contract::{
    ContractProperties, DataContract, FieldAllocator, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_kernels_geo::analyze::analyze_geo_contract;
use plenora_kernels_geo::arrow_adapter::{
    geo_metadata_json, DEFAULT_GEOMETRY_COLUMN, GEO_METADATA_KEY, GEOARROW_EXTENSION_KEY,
    GEOARROW_WKB_EXTENSION,
};
use serde_json::{json, Map, Value};

fn projected_crs() -> ResolvedCrs {
    ResolvedCrs::from_resolved_parts(
        "EPSG:32632".to_owned(),
        json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
        CrsKind::Projected,
        Some(1.0),
    )
}

fn geometry_arrow_field() -> Field {
    let mut metadata = HashMap::new();
    metadata.insert(
        GEOARROW_EXTENSION_KEY.to_owned(),
        GEOARROW_WKB_EXTENSION.to_owned(),
    );
    metadata.insert(
        GEO_METADATA_KEY.to_owned(),
        geo_metadata_json("EPSG:32632").expect("geo metadata"),
    );
    Field::new(DEFAULT_GEOMETRY_COLUMN, DataType::Binary, true).with_metadata(metadata)
}

fn geo_contract(crs: ResolvedCrs) -> DataContract {
    DataContract::new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("label", DataType::Utf8, true),
            geometry_arrow_field(),
        ])),
        vec![GeometryColumnContract {
            field_id: FieldId(2),
            name: DEFAULT_GEOMETRY_COLUMN.to_owned(),
            crs,
            dimensions: GeometryDimensions::Xy,
            nullable: true,
        }],
        Some(FieldId(2)),
        ContractProperties::default(),
    )
    .expect("contratto geometrico valido")
}

fn tabular_contract() -> DataContract {
    DataContract::tabular(Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("x", DataType::Float64, true),
        Field::new("y", DataType::Float64, true),
    ])))
}

/// WKB little-endian di POINT(1 2), per le config `other_wkb`.
const POINT_WKB_HEX: &str = "0101000000000000000000f03f0000000000000040";

/// (op, n. input, input tabellare?, config base valida).
fn cases() -> Vec<(&'static str, usize, bool, Value)> {
    vec![
        ("geo.centroid", 1, false, json!({})),
        ("geo.convex_hull", 1, false, json!({})),
        ("geo.envelope", 1, false, json!({})),
        ("geo.boundary", 1, false, json!({})),
        ("geo.buffer", 1, false, json!({"distance": 100.0})),
        ("geo.simplify", 1, false, json!({"tolerance": 0.5})),
        ("geo.translate", 1, false, json!({"x_offset": 1.0, "y_offset": 2.0})),
        ("geo.scale", 1, false, json!({"x_factor": 2.0, "y_factor": 2.0})),
        ("geo.rotate", 1, false, json!({"degrees": 90.0})),
        ("geo.snap_to_grid", 1, false, json!({"grid_size": 1.0})),
        ("geo.densify", 1, false, json!({"max_segment_length": 10.0})),
        ("geo.area", 1, false, json!({})),
        ("geo.length", 1, false, json!({})),
        ("geo.perimeter", 1, false, json!({})),
        ("geo.vertex_count", 1, false, json!({})),
        ("geo.to_wkt", 1, false, json!({})),
        ("geo.bounds_extractor", 1, false, json!({})),
        ("geo.geometry_diagnostics", 1, false, json!({})),
        ("geo.explode", 1, false, json!({})),
        ("geo.delaunay", 1, false, json!({})),
        ("geo.dissolve", 1, false, json!({})),
        ("geo.line_merge", 1, false, json!({})),
        ("geo.polygonize", 1, false, json!({})),
        ("geo.voronoi", 1, false, json!({})),
        ("geo.clean_topology", 1, false, json!({"snap_tolerance": 0.01})),
        ("geo.from_coords", 1, true, json!({})),
        ("geo.reproject", 1, false, json!({"target_crs": "EPSG:32632"})),
        ("geo.distance", 1, false, json!({"other_wkb": POINT_WKB_HEX})),
        ("geo.hausdorff_distance", 1, false, json!({"other_wkb": POINT_WKB_HEX})),
        ("geo.predicate_intersects", 1, false, json!({"other_wkb": POINT_WKB_HEX})),
        ("geo.predicate_contains", 1, false, json!({"other_wkb": POINT_WKB_HEX})),
        ("geo.split", 1, false, json!({"other_wkb": POINT_WKB_HEX})),
        ("geo.clip", 2, false, json!({})),
        ("geo.intersection", 2, false, json!({})),
        ("geo.union", 2, false, json!({})),
        ("geo.difference", 2, false, json!({})),
        ("geo.symmetric_difference", 2, false, json!({})),
    ]
}

fn merge(base: &Value, patch: Option<Value>) -> Value {
    let (Some(Value::Object(patch)), Value::Object(base)) = (patch, base) else {
        return base.clone();
    };
    let mut merged: Map<String, Value> = base.clone();
    for (key, value) in patch {
        if key.len() <= 64 {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

fuzz_target!(|payload: &[u8]| {
    let table = cases();
    let selector = payload.first().copied().unwrap_or_default() as usize;
    let (op, arity, tabular, base) = &table[selector % table.len()];
    let patch = serde_json::from_slice::<Value>(payload.get(1..).unwrap_or(&[])).ok();
    let config = merge(base, patch);
    let plan_crs = projected_crs();
    let input = if *tabular {
        tabular_contract()
    } else {
        geo_contract(plan_crs.clone())
    };
    let inputs: Vec<DataContract> = (0..*arity).map(|_| input.clone()).collect();
    let mut fields = FieldAllocator::default();
    if let Ok(output) =
        analyze_geo_contract(op, &inputs, &config, Some(&plan_crs), &mut fields)
    {
        let names: HashSet<&str> = output
            .schema
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect();
        assert_eq!(
            names.len(),
            output.schema.fields().len(),
            "schema con nomi duplicati da {op}"
        );
    }
});
