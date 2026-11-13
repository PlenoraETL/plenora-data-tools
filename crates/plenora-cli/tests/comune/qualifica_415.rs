//! Fixture comune alle superfici Rust, CLI e runtime, senza dipendenze esterne.
use plenora_core::arrow::array::Array;
use plenora_core::arrow::{
    array::{BinaryArray, Int64Array, RecordBatch},
    DataType, Field, Metadata, Schema,
};
use std::sync::Arc;

pub fn batch(extension: bool, semantics: &str, id: &str) -> RecordBatch {
    let mut md = Metadata::new();
    for (key, value) in [
        ("plenora.field_id", "3"),
        ("ARROW:extension:name", "geoarrow.wkb"),
        ("plenora.geometry.encoding", "wkb"),
        ("plenora.geometry.dimensions", "xy"),
        ("plenora.geometry.types_declaration", "exact"),
        ("plenora.geometry.types", "polygon"),
        ("plenora.geometry.crs_resolution", "resolved"),
        ("plenora.geometry.crs_id", "EPSG:32632"),
        ("plenora.geometry.axis_order", "easting_northing"),
        ("plenora.geometry.spatial_semantics", semantics),
        ("plenora.geometry.srid", "32632"),
        ("plenora.geometry.precision", "float64"),
        (
            "plenora.postgres.native_declaration",
            "geometry(Polygon,32632)",
        ),
        ("plenora.postgres.future", "SENTINEL_415"),
    ] {
        md.insert(key, value);
    }
    if extension {
        md.insert(
            "ARROW:extension:metadata",
            format!(
                r#"{{ "crs_type" : "authority_code", "crs" : "{id}", "edges" : "{}" }}"#,
                if semantics == "geography" {
                    "spherical"
                } else {
                    "planar"
                }
            ),
        );
    }
    let mut attr = Metadata::new();
    attr.insert("plenora.field_id", "1");
    attr.insert("plenora.postgres.native_type", "int8");
    let mut sm = Metadata::new();
    sm.insert("plenora.contract.version", "1");
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("id", DataType::Int64, false).with_metadata(attr),
            Field::new("g", DataType::Binary, true).with_metadata(md),
        ],
        sm,
    ));
    let outer = [
        (0.0, 0.0),
        (40.0, 0.0),
        (40.0, 40.0),
        (0.0, 40.0),
        (0.0, 0.0),
    ];
    let hole = [
        (10.0, 10.0),
        (10.0, 20.0),
        (20.0, 20.0),
        (20.0, 10.0),
        (10.0, 10.0),
    ];
    let triangle = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 0.0)];
    let wkb = polygon(&[&outer, &hole]);
    let triangle = polygon(&[&triangle]);

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(BinaryArray::from(vec![
                Some(wkb.as_slice()),
                Some(triangle.as_slice()),
                None,
            ])),
        ],
    )
    .unwrap()
}

pub fn plan(compute: bool) -> String {
    if compute {
        r#"{"version":1,"inputs":["t"],"steps":[{"out":"o","op":"geo.centroid","in":["t"],"config":{}}],"outputs":["o"]}"#.into()
    } else {
        r#"{"version":1,"inputs":["t"],"steps":[],"outputs":["t"]}"#.into()
    }
}

#[allow(
    clippy::suboptimal_flops,
    reason = "H-07 prescrive le operazioni float64 separate: FMA cambierebbe l'oracolo bitwise"
)]
pub fn check(input: &RecordBatch, output: &RecordBatch, compute: bool) {
    if !compute {
        assert_eq!(input, output);
        return;
    }
    assert_eq!(input.column(0).to_data(), output.column(0).to_data());
    assert_eq!(
        input.schema().field(0).metadata(),
        output.schema().field(0).metadata()
    );
    let schema = output.schema();
    let md = schema.field(1).metadata();
    assert_eq!(md.get("plenora.field_id").map(String::as_str), Some("3"));
    assert_eq!(
        md.get("plenora.geometry.types").map(String::as_str),
        Some("point")
    );
    assert!(!md.keys().any(|key| key.starts_with("plenora.postgres.")));
    // H-07: anelli traslati, pesatura float64 per area e divisione finale.
    // Attesa dichiarata dalla qualifica, non ricavata dall'uscita del kernel.
    let hole = |base: f64| ((base + 20.0) * 1600.0 - (base + 15.0) * 100.0) / 1500.0;
    let triangle = |base: f64, offset: f64| ((base + offset / 3.0) * 100.0) / 100.0;
    let bytes = [
        (hole(500_000.0), hole(4_650_000.0)),
        (triangle(500_000.0, 20.0), triangle(4_650_000.0, 10.0)),
    ]
    .map(|(x, y)| {
        let mut wkb = vec![1];
        wkb.extend(1_u32.to_le_bytes());
        wkb.extend(x.to_le_bytes());
        wkb.extend(y.to_le_bytes());
        wkb
    });
    let expected = BinaryArray::from(vec![
        Some(bytes[0].as_slice()),
        Some(bytes[1].as_slice()),
        None,
    ]);
    assert_eq!(output.column(1).to_data(), expected.to_data());
}

fn polygon(rings: &[&[(f64, f64)]]) -> Vec<u8> {
    let mut wkb = vec![1];
    wkb.extend(3_u32.to_le_bytes());
    wkb.extend(u32::try_from(rings.len()).unwrap().to_le_bytes());
    for ring in rings {
        wkb.extend(u32::try_from(ring.len()).unwrap().to_le_bytes());
        for (x, y) in *ring {
            wkb.extend((500_000.0 + x).to_le_bytes());
            wkb.extend((4_650_000.0 + y).to_le_bytes());
        }
    }
    wkb
}
