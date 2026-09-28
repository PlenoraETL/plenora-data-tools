//! Impalcature condivise dai test interni del crate.
//!
//! Qui sta solo la costruzione degli ingressi: schemi, contratti, batch. Mai un
//! atteso e mai un confronto: ogni prova ricava il proprio risultato per conto
//! suo. Dove due copie differivano, la differenza e' un parametro esplicito e
//! non una scelta fatta qui.

use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use plenora_core::arrow::schema::{DataType, Field, Schema, SchemaRef};
use plenora_core::contract::{
    ContractCrs, ContractProperties, DataContract, FieldId, GeometryColumnContract,
    GeometryDimensions,
};
use plenora_core::crs::{CrsKind, ResolvedCrs};
use plenora_core::Result;

use crate::executor::{execute, Input, Inputs, Output};
use crate::planner::validate;
use crate::prepare::RuntimeContext;

// ---------------------------------------------------------------------------
// Contratti
// ---------------------------------------------------------------------------

/// `EPSG:32632` risolto e proiettato, con lo stub PROJJSON delle fixture.
pub fn projected_crs() -> ResolvedCrs {
    ResolvedCrs::from_resolved_parts(
        "EPSG:32632".to_owned(),
        json!({"type": "ProjectedCRS", "name": "WGS 84 / UTM zone 32N"}),
        CrsKind::Projected,
        Some(1.0),
    )
}

/// Schema tabellare a due colonne: `id` Int64 obbligatorio, `name` Utf8
/// annullabile.
pub fn table_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]))
}

/// Il contratto tabellare di [`table_schema`].
pub fn table_contract() -> DataContract {
    DataContract::tabular(table_schema())
}

/// Campo geometria WKB con il solo marcatore di estensione `geoarrow.wkb`:
/// basta a rendere la colonna identificabile dal check di analyze
/// (piano-v5.md#contratti-di-input, decisione 8).
pub fn wkb_geometry_field(name: &str) -> Field {
    Field::new(name, DataType::Binary, true).with_metadata(std::collections::HashMap::from([(
        plenora_kernels_geo::arrow_adapter::GEOARROW_EXTENSION_KEY.to_owned(),
        plenora_kernels_geo::arrow_adapter::GEOARROW_WKB_EXTENSION.to_owned(),
    )]))
}

/// Schema `id` + `geom` con il campo di [`wkb_geometry_field`].
pub fn wkb_geo_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        wkb_geometry_field("geom"),
    ]))
}

/// Contratto con una sola geometria `geom`, XY, annullabile e senza tipi
/// dichiarati. Schema, `FieldId` e CRS sono cio' in cui le fixture dei
/// moduli differiscono, e restano scelte del chiamante.
pub fn geo_contract_con(schema: SchemaRef, field_id: u32, crs: ContractCrs) -> DataContract {
    DataContract::new(
        schema,
        vec![GeometryColumnContract {
            field_id: FieldId(field_id),
            name: "geom".to_owned(),
            crs,
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

// ---------------------------------------------------------------------------
// Esecuzione
// ---------------------------------------------------------------------------

/// Un batch di [`table_schema`], senza nulli.
pub fn table_batch(ids: &[i64], names: &[&str]) -> RecordBatch {
    RecordBatch::try_new(
        table_schema(),
        vec![
            Arc::new(Int64Array::from(ids.to_vec())) as ArrayRef,
            Arc::new(StringArray::from(
                names.iter().map(|n| Some(*n)).collect::<Vec<_>>(),
            )) as ArrayRef,
        ],
    )
    .expect("batch fixture valido")
}

/// Valida il piano sui contratti ed esegue con il contesto di default.
pub fn run(
    plan: &serde_json::Value,
    inputs: Inputs,
    contracts: &[(String, DataContract)],
) -> Result<Output> {
    let graph = validate(&plan.to_string(), contracts)?;
    execute(&graph, inputs, RuntimeContext::default())
}

/// `Inputs` SENZA contratto: il percorso permissivo, deprecato ma ancora
/// supportato e quindi ancora da testare. L'`allow(deprecated)` sta sulla
/// dichiarazione del modulo, in `lib.rs`.
pub fn single_input(name: &str, batches: Vec<RecordBatch>) -> Inputs {
    Inputs::new()
        .with(name, Input::from_batches(batches).expect("input non vuoto"))
        .expect("input unico")
}
