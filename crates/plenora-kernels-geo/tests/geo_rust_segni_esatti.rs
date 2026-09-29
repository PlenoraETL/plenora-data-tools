//! Orientamento e annidamento delle facce decisi da segni **esatti**.
//!
//! Il kernel del laboratorio classificava una faccia dal segno di un'area
//! calcolata in `f64` senza compensazione: il quadrato unitario con vertice
//! in `(2^30, 2^30)` ha area calcolata `0`, finiva fra gli anelli invalidi e,
//! con `require_complete = false` (il default), l'operazione riusciva senza
//! il poligono. Questi casi passano dall'API pubblica e dall'adapter Arrow.

use std::sync::Arc;

use geo::{Area, Coord, Geometry, LineString, MultiLineString, Polygon};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{Array, BinaryArray, Int64Array, StringArray};
use plenora_core::arrow::{DataType, Field, RecordBatch, Schema, SchemaRef};
use plenora_core::contract::arrow_metadata::{geometry_output_field, DEFAULT_GEOMETRY_COLUMN};
use plenora_kernels_geo::rust_backend::arrow::{
    polygonize_batches, split_batches, PolygonizeParams, CLASS_COLUMN,
};
use plenora_kernels_geo::rust_backend::precision::Precision;
use plenora_kernels_geo::rust_backend::{polygonize_linework, split_polygon_by_linework};

const LIMIT: u64 = 1_000_000;

/// Precisione dei test in unita' astratte, compatibile con il modulo delle
/// coordinate: i kernel rifiutano una precisione piu' fine di 64 volte la
/// spaziatura dei `f64` al modulo massimo (`PrecisionInsufficient`). Qui la
/// piu' fine ammessa con margine, almeno un milionesimo: i casi provano i
/// segni esatti, non la politica del centimetro.
fn precisione_per(modulo: f64) -> Precision {
    let modulo = modulo.abs();
    let spaziatura = f64::from_bits(modulo.to_bits() + 1) - modulo;
    Precision::new((128.0 * spaziatura).max(1e-6)).expect("precisione")
}
const CRS: &str = "EPSG:3857";

/// Il quadrato `[x, x+lato] x [y, y+lato]` chiuso, antiorario o orario, a
/// partire dal vertice `inizio`.
fn quadrato(x: f64, y: f64, lato: f64, orario: bool, inizio: usize) -> LineString<f64> {
    let mut vertici = [
        Coord { x, y },
        Coord { x: x + lato, y },
        Coord {
            x: x + lato,
            y: y + lato,
        },
        Coord { x, y: y + lato },
    ];
    if orario {
        vertici.reverse();
    }
    let mut anello: Vec<_> = (0..4).map(|i| vertici[(inizio + i) % 4]).collect();
    anello.push(anello[0]);
    LineString::new(anello)
}

/// Offset, lato e area attesa: il caso della revisione, traslazioni grandi
/// di segno diverso, aree minuscole.
fn casi() -> Vec<(f64, f64, f64)> {
    let mut casi = Vec::new();
    for (x, y) in [
        (2_f64.powi(30), 2_f64.powi(30)),
        (2_f64.powi(40), -(2_f64.powi(41))),
        (-(2_f64.powi(45)), 2_f64.powi(44)),
        (1.0e15, 1.0e15),
    ] {
        casi.push((x, y, 1.0));
    }
    // Aree minuscole: lato uguale all'ULP dell'offset.
    casi.push((2_f64.powi(20), 2_f64.powi(20), 2_f64.powi(-32)));
    casi.push((2_f64.powi(-10), 2_f64.powi(-10), 2_f64.powi(-60)));
    casi
}

#[test]
fn il_quadrato_lontano_dall_origine_resta_un_poligono() {
    for (x, y, lato) in casi() {
        for orario in [false, true] {
            for inizio in 0..4 {
                let anello = quadrato(x, y, lato, orario, inizio);
                for node_input in [true, false] {
                    let risultato = polygonize_linework(
                        &Geometry::LineString(anello.clone()),
                        node_input,
                        true,
                        LIMIT,
                        LIMIT,
                        LIMIT,
                        LIMIT,
                        precisione_per(x.abs().max(y.abs()) + lato),
                    )
                    .unwrap_or_else(|errore| {
                        panic!("({x}, {y}, {lato}) orario={orario} inizio={inizio}: {errore}")
                    });
                    assert_eq!(risultato.polygons.len(), 1, "({x}, {y}, {lato})");
                    let area = risultato.polygons[0].unsigned_area();
                    let attesa = lato * lato;
                    assert!(
                        (area - attesa).abs() <= attesa * 1e-12,
                        "({x}, {y}, {lato}): area {area}"
                    );
                }
            }
        }
    }
}

#[test]
fn il_quadrato_con_buco_lontano_dall_origine_conserva_il_buco() {
    let a = 2_f64.powi(30);
    let esterno = quadrato(a, a, 4.0, false, 0);
    let interno = quadrato(a + 1.0, a + 1.0, 1.0, true, 2);
    let risultato = polygonize_linework(
        &Geometry::MultiLineString(MultiLineString::new(vec![esterno, interno])),
        true,
        true,
        LIMIT,
        LIMIT,
        LIMIT,
        LIMIT,
        precisione_per(a + 4.0),
    )
    .expect("polygonize");
    assert_eq!(risultato.polygons.len(), 2);
    let mut aree: Vec<f64> = risultato.polygons.iter().map(Area::unsigned_area).collect();
    aree.sort_by(f64::total_cmp);
    assert_eq!(aree, [1.0, 15.0]);
}

#[test]
fn split_lontano_dall_origine() {
    let a = 2_f64.powi(30);
    let sorgente = Geometry::Polygon(Polygon::new(quadrato(a, a, 2.0, false, 0), Vec::new()));
    let lama = Geometry::LineString(LineString::from(vec![
        (a + 1.0, a - 1.0),
        (a + 1.0, a + 3.0),
    ]));
    let parti = split_polygon_by_linework(
        &sorgente,
        &lama,
        LIMIT,
        LIMIT,
        LIMIT,
        LIMIT,
        precisione_per(a + 4.0),
    )
    .expect("split");
    assert_eq!(parti.len(), 2);
}

fn tabella(celle: &[Vec<u8>]) -> (SchemaRef, RecordBatch) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        geometry_output_field(DEFAULT_GEOMETRY_COLUMN, CRS).expect("campo"),
    ]));
    let ids = (0..celle.len())
        .map(|riga| i64::try_from(riga).expect("id"))
        .collect::<Vec<_>>();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(ids)),
            Arc::new(
                celle
                    .iter()
                    .map(|cella| Some(cella.as_slice()))
                    .collect::<BinaryArray>(),
            ),
        ],
    )
    .expect("batch");
    (schema, batch)
}

#[test]
fn l_adapter_arrow_classifica_il_quadrato_come_poligono() {
    for (x, y, lato) in casi() {
        for orario in [false, true] {
            let cella = Geometry::LineString(quadrato(x, y, lato, orario, 1))
                .to_wkb(CoordDimensions::xy())
                .expect("wkb");
            let (schema, batch) = tabella(&[cella]);
            let (schema_uscita, batches) = polygonize_batches(
                &schema,
                &[batch],
                DEFAULT_GEOMETRY_COLUMN,
                CRS,
                PolygonizeParams::default(),
                LIMIT,
                precisione_per(x.abs().max(y.abs()) + lato),
            )
            .expect("polygonize");
            let classi = batches[0]
                .column(schema_uscita.index_of(CLASS_COLUMN).expect("classe"))
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8");
            let classi: Vec<&str> = (0..classi.len()).map(|riga| classi.value(riga)).collect();
            assert_eq!(classi, ["polygon"], "({x}, {y}, {lato}) orario={orario}");
        }
    }

    let a = 2_f64.powi(30);
    let sorgente = Geometry::Polygon(Polygon::new(quadrato(a, a, 2.0, true, 3), Vec::new()))
        .to_wkb(CoordDimensions::xy())
        .expect("wkb");
    let lama = Geometry::LineString(LineString::from(vec![
        (a + 1.0, a - 1.0),
        (a + 1.0, a + 3.0),
    ]))
    .to_wkb(CoordDimensions::xy())
    .expect("wkb");
    let (schema, batch) = tabella(&[sorgente]);
    let splitters = std::iter::once(Some(lama.as_slice())).collect::<BinaryArray>();
    let (_, batches) = split_batches(
        &schema,
        &[batch],
        DEFAULT_GEOMETRY_COLUMN,
        &splitters,
        CRS,
        None,
        LIMIT,
        precisione_per(a + 4.0),
    )
    .expect("split");
    assert_eq!(batches[0].num_rows(), 2);
}
