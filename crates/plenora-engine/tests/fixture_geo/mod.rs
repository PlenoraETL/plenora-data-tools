//! Le fixture geometriche condivise dai due oracoli.
//!
//! # Che cosa sta qui, e che cosa deliberatamente no
//!
//! Qui c'e' **solo la costruzione dell'ingresso**: lo schema, il contratto, i
//! WKB e i batch. Sono dati, e due copie degli stessi dati non provano nulla
//! di piu' di una — provano solo che qualcuno le ha tenute allineate.
//!
//! Non stanno qui, e non devono arrivarci:
//!
//! - il **calcolo dell'atteso**. Ogni oracolo lo ricava per conto suo, perche'
//!   e' cio' che sta verificando;
//! - il **percorso di confronto**: `error_signature`, `variant_name`,
//!   l'estrazione del primo errore da uno stream, gli `assert` che affiancano
//!   il percorso generico e quello ottimizzato. Condividerli farebbe
//!   concordare i due file per costruzione, ed e' esattamente cio' che due
//!   implementazioni ugualmente sbagliate farebbero da sole.
//!
//! Il confine non e' una preferenza di stile: un oracolo che condivide il modo
//! di decidere con cio' che deve giudicare ha smesso di essere un oracolo.

use std::sync::Arc;

use geo::{Geometry, Point};
use geozero::{CoordDimensions, ToWkb};
use plenora_core::arrow::array::{ArrayRef, BinaryArray, Int64Array, RecordBatch};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use serde_json::json;

/// Schema a due colonne: `id` e la geometria in `EPSG:32632`.
pub fn geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        plenora_kernels_geo::arrow_adapter::geometry_output_field("geom", "EPSG:32632")
            .expect("campo geometria"),
    ]))
}

/// Il contratto che dichiara quella geometria come CRS risolto e proiettato.
pub fn geo_contract() -> DataContract {
    DataContract::new(
        geo_schema(),
        vec![GeometryColumnContract {
            field_id: FieldId(3),
            name: "geom".to_owned(),
            crs: ContractCrs::Resolved(ResolvedCrs::from_resolved_parts(
                "EPSG:32632".to_owned(),
                json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
                CrsKind::Projected,
                Some(1.0),
            )),
            dimensions: GeometryDimensions::Xy,
            encoding: None,
            nullable: true,
            types: GeometryColumnContract::undeclared_types(),
        }],
        None,
        ContractProperties::default(),
    )
    .expect("contratto fixture valido")
}

/// Una geometria in WKB, due dimensioni.
pub fn to_wkb(geometry: &Geometry<f64>) -> Vec<u8> {
    geometry.to_wkb(CoordDimensions::xy()).expect("wkb fixture")
}

/// Un punto in WKB.
pub fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    to_wkb(&Geometry::Point(Point::new(x, y)))
}

/// Un batch con gli identificatori e le celle dichiarati.
///
/// Le celle sono `Option`: un `None` e' una geometria assente, che e' un caso
/// e non un errore di costruzione.
pub fn geo_batch(ids: &[i64], cells: &[Option<Vec<u8>>]) -> RecordBatch {
    let refs: Vec<Option<&[u8]>> = cells.iter().map(|cell| cell.as_deref()).collect();
    RecordBatch::try_new(
        geo_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(BinaryArray::from(refs)) as ArrayRef,
        ],
    )
    .expect("batch geo fixture valido")
}
