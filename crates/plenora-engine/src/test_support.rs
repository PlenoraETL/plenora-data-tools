//! Impalcature condivise dai test interni del crate.
//!
//! Qui sta solo la costruzione degli ingressi: schemi, contratti, batch. Mai un
//! atteso e mai un confronto: ogni prova ricava il proprio risultato per conto
//! suo. Dove due copie differivano, la differenza e' un parametro esplicito e
//! non una scelta fatta qui.

use std::io::Read;
use std::sync::Arc;

use serde_json::json;

use plenora_core::arrow::array::{Array, ArrayRef, Int64Array, RecordBatch, StringArray};
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
// Lettura dei batch
// ---------------------------------------------------------------------------

/// Accesso tipizzato alle colonne nelle prove: un tipo diverso da quello
/// atteso e' un panico, mai un `None` lasciato passare.
pub trait ColonnaTipizzata {
    /// La colonna in posizione `index`, con il tipo concreto `A`.
    fn colonna_a<A: Array + 'static>(&self, index: usize) -> &A;
}

impl ColonnaTipizzata for RecordBatch {
    fn colonna_a<A: Array + 'static>(&self, index: usize) -> &A {
        self.column(index)
            .as_any()
            .downcast_ref::<A>()
            .unwrap_or_else(|| panic!("colonna {index}: tipo inatteso"))
    }
}

// ---------------------------------------------------------------------------
// Sorgenti `Read` finte
// ---------------------------------------------------------------------------

/// Sorgente che dichiara molto e consegna poco, **e registra ogni richiesta
/// di lettura**: sono le taglie richieste, non l'esito, a dire se un lettore
/// ha dimensionato il buffer sul dichiarato.
pub struct SorgenteTroncata<'a> {
    byte: &'a [u8],
    letto: usize,
    /// La taglia di ogni slice passata a `read`, nell'ordine.
    pub richieste: Vec<usize>,
    interrompi_una_volta: bool,
}

impl<'a> SorgenteTroncata<'a> {
    /// Consegna `byte` e poi la fine del flusso.
    pub const fn nuova(byte: &'a [u8]) -> Self {
        Self {
            byte,
            letto: 0,
            richieste: Vec::new(),
            interrompi_una_volta: false,
        }
    }

    /// Come [`Self::nuova`], ma la prima lettura fallisce con `Interrupted`
    /// senza essere registrata.
    pub const fn interrotta_una_volta(byte: &'a [u8]) -> Self {
        Self {
            byte,
            letto: 0,
            richieste: Vec::new(),
            interrompi_una_volta: true,
        }
    }
}

impl Read for SorgenteTroncata<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.interrompi_una_volta {
            self.interrompi_una_volta = false;
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "interrotta",
            ));
        }
        self.richieste.push(out.len());
        let resto = self.byte.len().saturating_sub(self.letto);
        let quanti = resto.min(out.len());
        out[..quanti].copy_from_slice(&self.byte[self.letto..self.letto + quanti]);
        self.letto += quanti;
        Ok(quanti)
    }
}

/// Sorgente che serve per intero i primi `onesti` byte, poi mente: dichiara
/// un byte piu' della fetta senza averla riempita.
///
/// `onesti` e' la parte che il lettore legge con `read_exact`, che
/// rifiuterebbe subito una dichiarazione in eccesso: la bugia deve arrivare
/// alla prima fetta del payload, e dove cada dipende dal formato.
pub struct SorgenteBugiarda<'a> {
    byte: &'a [u8],
    letto: usize,
    onesti: usize,
}

impl<'a> SorgenteBugiarda<'a> {
    /// Mente dopo i primi `onesti` byte di `byte`.
    pub const fn nuova(byte: &'a [u8], onesti: usize) -> Self {
        Self {
            byte,
            letto: 0,
            onesti,
        }
    }
}

impl Read for SorgenteBugiarda<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let resto = self.byte.len().saturating_sub(self.letto);
        let quanti = resto.min(out.len());
        out[..quanti].copy_from_slice(&self.byte[self.letto..self.letto + quanti]);
        self.letto += quanti;
        if self.letto <= self.onesti {
            return Ok(quanti);
        }
        Ok(out.len() + 1)
    }
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
